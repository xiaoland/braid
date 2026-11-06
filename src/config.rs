use std::{
    collections::{HashMap, HashSet},
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

use crate::home::validate_instance_key;

pub const CONFIG_SCHEMA_VERSION: u32 = 3;
/// Process-local credential injection for one Pi provider. Users configure only
/// the secret file; native materials reference this fixed name.
pub const PROVIDER_API_KEY_ENVIRONMENT: &str = "BRAID_PROVIDER_API_KEY";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read config {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("unsupported config schema {found}; this binary supports {supported}")]
    Schema { found: u32, supported: u32 },
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceConfig {
    pub key: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(skip)]
    pub(crate) finite_run: Option<crate::run::RunContext>,
    pub schema_version: u32,
    pub instance: InstanceConfig,
    pub runtime: RuntimeConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GitHubConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<LocalConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builtin: Option<BuiltinConfig>,
    #[serde(skip)]
    pub loaded_from: PathBuf,
    pub scheduler: SchedulerConfig,
    /// Adapter connectivity, keyed by `adapter_type`. One entry per `adapter_type`
    /// per worker; the profile's `adapter_type` locates the runtime entry.
    pub runtimes: Vec<RuntimeEntry>,
    /// LLM provider catalogue. Only Pi profiles reference these entries; Codex
    /// configurations may omit the catalogue entirely.
    #[serde(default)]
    pub llm_providers: Vec<LlmProvider>,
    pub tools: ToolConfig,
    pub server: ServerConfig,
    pub telemetry: TelemetryConfig,
    pub profiles: Vec<Profile>,
    pub profile_selection: ProfileSelection,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Defaults to `<config_dir>/state`.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Defaults to `<root>/braid.sqlite3`.
    #[serde(default)]
    pub database: Option<PathBuf>,
    /// Defaults to `<root>/backups`.
    #[serde(default)]
    pub backups: Option<PathBuf>,
    /// Where generation-scoped Agent worktrees are provisioned. Defaults to
    /// `<root>/worktrees`. Only new provisioning uses it; existing
    /// generations keep the worktree paths recorded in `SQLite`.
    #[serde(default)]
    pub worktrees: Option<PathBuf>,
    #[serde(default)]
    pub auto_migrate: bool,
}

impl RuntimeConfig {
    /// Fill omitted/relative paths using `base` (the directory containing the
    /// loaded config file) and normalize relative paths to absolute paths.
    pub fn resolve(&mut self, base: &Path) {
        if self.root.is_none() {
            self.root = Some(base.join("state"));
        }
        let root = self.root.clone().expect("root resolved above");
        let root = resolve_path(base, &root);
        if self.database.is_none() {
            self.database = Some(root.join("braid.sqlite3"));
        }
        if self.backups.is_none() {
            self.backups = Some(root.join("backups"));
        }
        if self.worktrees.is_none() {
            self.worktrees = Some(root.join("worktrees"));
        }
        self.root = Some(root);
        self.database =
            Some(resolve_path(base, self.database.as_ref().expect("database resolved")));
        self.backups = Some(resolve_path(base, self.backups.as_ref().expect("backups resolved")));
        self.worktrees =
            Some(resolve_path(base, self.worktrees.as_ref().expect("worktrees resolved")));
    }

    fn require_resolved(&self) {
        assert!(
            self.root.is_some()
                && self.database.is_some()
                && self.backups.is_some()
                && self.worktrees.is_some(),
            "RuntimeConfig paths must be resolved before use"
        );
    }

    pub fn root(&self) -> &Path {
        self.require_resolved();
        self.root.as_ref().expect("resolved")
    }

    pub fn database(&self) -> &Path {
        self.require_resolved();
        self.database.as_ref().expect("resolved")
    }

    pub fn backups(&self) -> &Path {
        self.require_resolved();
        self.backups.as_ref().expect("resolved")
    }

    pub fn worktrees(&self) -> &Path {
        self.require_resolved();
        self.worktrees.as_ref().expect("resolved")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEntry {
    /// Braid Pi material package manifest; absent for an external base Pi.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_runtime: Option<PathBuf>,
    pub adapter_type: String,
    /// Contract version checked at setup and serve time.
    pub version: String,
    /// Adapter-defined connectivity. Examples: local executable path, HTTP
    /// runtime API URL, runtime home directory.
    pub executable: PathBuf,
    #[serde(default)]
    pub api_url: Option<String>,
    #[serde(default)]
    pub home: Option<PathBuf>,
    /// Codex-specific schema pins, produced by the adapter verifier.
    #[serde(default)]
    pub stable_schema_sha256: Option<String>,
    #[serde(default)]
    pub experimental_schema_sha256: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum CredentialKind {
    #[serde(rename = "api_key")]
    ApiKey,
    #[serde(rename = "oauth")]
    OAuth,
    #[serde(rename = "none")]
    None,
}

/// Credential reference for one LLM provider. The referenced file is Braid
/// schema (never a runtime-native file); adapters convert it at
/// materialization time.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredential {
    pub kind: CredentialKind,
    /// Required for `api_key` and `oauth`; must be absent for `none`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
}

impl ProviderCredential {
    pub fn none() -> Self {
        Self { kind: CredentialKind::None, file: None }
    }

    pub fn load(&self) -> Result<LoadedCredential, ConfigError> {
        match self.kind {
            CredentialKind::None => {
                if self.file.is_some() {
                    return Err(ConfigError::Invalid(
                        "credential kind \"none\" must not reference a file".into(),
                    ));
                }
                Ok(LoadedCredential::None)
            }
            CredentialKind::ApiKey => {
                let file = self.file.as_ref().ok_or_else(|| {
                    ConfigError::Invalid("credential kind \"api_key\" requires a file".into())
                })?;
                Ok(LoadedCredential::ApiKey(load_provider_secret_file(file)?))
            }
            CredentialKind::OAuth => {
                let file = self.file.as_ref().ok_or_else(|| {
                    ConfigError::Invalid("credential kind \"oauth\" requires a file".into())
                })?;
                Ok(LoadedCredential::OAuth(load_oauth_file(file)?))
            }
        }
    }
}

pub enum LoadedCredential {
    ApiKey(String),
    OAuth(OAuthCredentialFile),
    None,
}

/// Braid-schema OAuth token set for one provider (no refresh lifecycle:
/// the operator replaces the file explicitly).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthCredentialFile {
    pub access: String,
    pub refresh: String,
    /// Expiry as epoch milliseconds, matching the provider's token record.
    pub expires: u64,
    /// Optional diagnostic account label; never required for rendering.
    #[serde(default)]
    pub account: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmProvider {
    pub id: String,
    /// Explicit credential for this provider. Schema 3 requires it; schema 2
    /// may omit it and rely on the legacy `api_key_file` fallback instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<ProviderCredential>,
    /// Provider endpoint rendered into Pi's models.json by the adapter.
    /// Required for providers referenced by a schema-3 Pi Profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Wire API name rendered into Pi's models.json (e.g. openai-completions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    /// Legacy schema-2 metadata. Pi's native models.json owns wire routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    /// Legacy schema-2 API key reference; schema 3 requires `credential`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_file: Option<PathBuf>,
    /// Declared models rendered into Pi's models.json by the adapter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<LlmModel>,
    /// Legacy schema-2 allowance metadata; Braid does not consume it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowances: Vec<LlmAllowance>,
}

impl LlmProvider {
    /// The credential this provider uses at runtime: the explicit
    /// `credential` entry, or the legacy schema-2 `api_key_file` fallback,
    /// otherwise an explicit keyless route.
    pub fn resolved_credential(&self) -> ProviderCredential {
        if let Some(credential) = &self.credential {
            return credential.clone();
        }
        if let Some(file) = &self.api_key_file {
            return ProviderCredential { kind: CredentialKind::ApiKey, file: Some(file.clone()) };
        }
        ProviderCredential::none()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmModel {
    /// Model id; legacy schema-2 configs used `model_id`.
    #[serde(alias = "model_id")]
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub thinking_level_map: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compat: Option<LlmModelCompat>,
    /// Legacy schema-2 cost metadata; schema 3 rejects it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_cost: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_input_cost: Option<f64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmModelCompat {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_store: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_developer_role: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_reasoning_effort: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_strict_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens_field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zai_tool_stream: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmAllowance {
    pub since: String,
    pub until: String,
    #[serde(default)]
    pub amount: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BuiltinConfig {
    pub repository: String,
    pub endpoint: Url,
}

// Parsed only to provide an actionable migration error for the previous declaration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalConfig {
    pub repository: String,
    pub source: PathBuf,
    pub default_branch: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubConfig {
    pub app_id: u64,
    pub repository: String,
    /// Additional repositories served by this same GitHub App and instance.
    /// The existing `repository` remains the single-repository default; when
    /// this list is non-empty every Profile must declare its repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_repositories: Vec<GitHubRepositoryConfig>,
    pub handle: String,
    pub api_version: String,
    pub private_key_file: PathBuf,
    #[serde(default)]
    pub webhook_secret_environment: Option<String>,
    #[serde(default)]
    pub webhook_secret_file: Option<PathBuf>,
    pub projects_v2_enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GitHubRepositoryConfig {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SchedulerConfig {
    pub quiet_seconds: u64,
    pub event_threshold: u32,
    pub reconciliation_seconds: u64,
}

/// Legacy connectivity shape, preserved as the adapter-internal contract while
/// the config surface moves to [[`runtimes`]] + [[`llm_providers`]].
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub codex: Option<CodexConfig>,
    pub pi: Option<PiConfig>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodexConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub braid_config_path: Option<PathBuf>,
    pub executable: PathBuf,
    pub home: PathBuf,
    pub version: String,
    pub stable_schema_sha256: String,
    pub experimental_schema_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PiConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_runtime: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub braid_config_path: Option<PathBuf>,
    pub executable: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(default = "ProviderCredential::none")]
    pub credential: ProviderCredential,
    pub thinking: Option<String>,
    pub home: Option<PathBuf>,
    #[serde(default)]
    pub profile_material_digest: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolConfig {
    pub git: PathBuf,
    pub gh: PathBuf,
    pub wrangler: PathBuf,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub ingress: SocketAddr,
    pub health: SocketAddr,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    pub endpoint: Url,
    pub sample_ratio: f64,
    #[serde(default)]
    pub incident_mode: bool,
    pub export_timeout_seconds: u64,
    pub service_name: String,
    #[serde(default = "default_log_format")]
    pub log_format: LogFormat,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    Text,
    Json,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: String,
    pub display_name: String,
    pub tags: Vec<String>,
    /// Locates the runtime entry that implements this profile's adapter.
    pub adapter_type: String,
    /// Contract pin checked against the runtime entry's version.
    pub adapter_version: String,
    /// References an entry in `llm_providers` for adapters that need an LLM.
    /// Adapters with their own authentication, such as Codex, omit this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// GitHub repository this Profile serves. Single-repository GitHub configs
    /// may omit it and use `github.repository`; multi-repository configs must
    /// set it explicitly. Built-in platform Profiles omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub user_instructions: String,
    /// Explicit Pi-discoverable skill entrypoints: a `SKILL.md` file, one skill
    /// directory, or a shallow catalog of skill directories. Complete trees bind.
    #[serde(default)]
    pub skills: Vec<PathBuf>,
    /// Pi-native semantic materials (`models.json`, `settings.json`, `agents/*.md`).
    #[serde(default)]
    pub native_template: Option<PathBuf>,
    /// The source Git checkout for this instance's repository, shared by all
    /// Profiles as the worktree provisioning source (one repository = one
    /// source checkout). Defaults to `<config_dir>/source`; Agent sessions
    /// never edit it directly — they run in provisioned worktrees.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    pub github_actor_node_id: Option<String>,
    pub status_surfaces: Vec<String>,
    pub github_context_soft_ratio: f64,
    pub github_context_hard_bytes: usize,
}

impl Profile {
    /// Fill an omitted workspace with the instance source checkout and
    /// normalize a relative override against the config directory.
    pub fn resolve_workspace(&mut self, base: &Path) {
        let workspace = self.workspace.take().unwrap_or_else(|| base.join("source"));
        self.workspace = Some(resolve_path(base, &workspace));
        for skill in &mut self.skills {
            *skill = resolve_path(base, skill);
        }
        if let Some(template) = &mut self.native_template {
            *template = resolve_path(base, template);
        }
    }

    /// The resolved source checkout path (valid after `Config::load`).
    pub fn workspace(&self) -> &Path {
        self.workspace.as_deref().expect("Profile workspace resolved before use")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSelection {
    pub default_pr_profile: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigSummary<'a> {
    pub schema_version: u32,
    pub upstream: &'static str,
    pub instance_key: &'a str,
    pub repository: &'a str,
    pub repositories: Vec<&'a str>,
    pub runtime_root: &'a Path,
    pub database: &'a Path,
    pub profile_ids: Vec<&'a str>,
    pub default_pr_profile: &'a str,
    pub trace_sample_ratio: f64,
    pub incident_mode: bool,
    pub effective_trace_sample_ratio: f64,
}

/// Secrets file referenced by `github.webhook_secret_file`.
#[derive(Debug, Deserialize)]
pub struct WebhookSecretFile {
    pub webhook_secret: String,
}

/// Secrets file referenced by `[[llm_providers]].api_key_file`.
#[derive(Debug, Deserialize)]
pub struct ProviderSecretFile {
    pub provider_api_key: String,
}

fn load_webhook_secret_file(path: &Path) -> Result<String, ConfigError> {
    let text = fs::read_to_string(path)
        .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
    let deserializer = toml::de::Deserializer::parse(&text).map_err(|error| {
        ConfigError::Invalid(format!("cannot parse secrets file {}: {error}", path.display()))
    })?;
    let file: WebhookSecretFile =
        serde_path_to_error::deserialize(deserializer).map_err(|error| {
            ConfigError::Invalid(format!("cannot parse secrets file {}: {error}", path.display()))
        })?;
    Ok(file.webhook_secret)
}

pub(crate) fn load_provider_secret_file(path: &Path) -> Result<String, ConfigError> {
    let text = fs::read_to_string(path)
        .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
    let deserializer = toml::de::Deserializer::parse(&text).map_err(|error| {
        ConfigError::Invalid(format!("cannot parse secrets file {}: {error}", path.display()))
    })?;
    let file: ProviderSecretFile =
        serde_path_to_error::deserialize(deserializer).map_err(|error| {
            ConfigError::Invalid(format!("cannot parse secrets file {}: {error}", path.display()))
        })?;
    Ok(file.provider_api_key)
}

pub(crate) fn load_oauth_file(path: &Path) -> Result<OAuthCredentialFile, ConfigError> {
    let text = fs::read_to_string(path)
        .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
    let deserializer = toml::de::Deserializer::parse(&text).map_err(|error| {
        ConfigError::Invalid(format!(
            "cannot parse OAuth credential file {}: {error}",
            path.display()
        ))
    })?;
    let file: OAuthCredentialFile =
        serde_path_to_error::deserialize(deserializer).map_err(|error| {
            ConfigError::Invalid(format!(
                "cannot parse OAuth credential file {}: {error}",
                path.display()
            ))
        })?;
    if file.access.trim().is_empty() || file.refresh.trim().is_empty() {
        return Err(ConfigError::Invalid(format!(
            "OAuth credential file {} requires non-empty access and refresh tokens",
            path.display()
        )));
    }
    Ok(file)
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path)
            .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
        let deserializer = toml::de::Deserializer::parse(&text).map_err(|error| {
            ConfigError::Invalid(format!("cannot parse TOML config {}: {error}", path.display()))
        })?;
        let mut config: Self = serde_path_to_error::deserialize(deserializer).map_err(|error| {
            ConfigError::Invalid(format!("cannot parse TOML config {}: {error}", path.display()))
        })?;
        let base = path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let base = if base.is_absolute() {
            base
        } else {
            std::env::current_dir().map(|cwd| cwd.join(&base)).unwrap_or(base)
        };
        config.loaded_from = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?
                .join(path)
        };
        config.runtime.resolve(&base);
        for runtime in &mut config.runtimes {
            if let Some(manifest) = &mut runtime.native_runtime {
                *manifest = resolve_path(&base, manifest);
            }
        }
        let multi_repository =
            config.github.as_ref().is_some_and(|github| !github.additional_repositories.is_empty());
        for profile in &mut config.profiles {
            if multi_repository && profile.workspace.is_none() {
                return Err(ConfigError::Invalid(format!(
                    "profile {:?} requires an explicit workspace in a multi-repository config",
                    profile.id
                )));
            }
            profile.resolve_workspace(&base);
        }
        config.validate()?;
        Ok(config)
    }

    #[allow(dead_code)]
    pub fn instance_key(&self) -> &str {
        &self.instance.key
    }

    pub fn repository(&self) -> &str {
        match (&self.github, &self.builtin) {
            (Some(github), None) => &github.repository,
            (None, Some(builtin)) => &builtin.repository,
            _ => unreachable!("configuration validates exactly one collaboration backend"),
        }
    }

    pub fn is_builtin(&self) -> bool {
        self.builtin.is_some()
    }

    pub fn github_repositories(&self) -> Vec<&str> {
        let Some(github) = &self.github else { return Vec::new() };
        std::iter::once(github.repository.as_str())
            .chain(github.additional_repositories.iter().map(|repository| repository.name.as_str()))
            .collect()
    }

    pub fn repositories(&self) -> Vec<&str> {
        if self.is_builtin() { vec![self.repository()] } else { self.github_repositories() }
    }

    pub fn is_multi_repository(&self) -> bool {
        self.github.as_ref().is_some_and(|github| !github.additional_repositories.is_empty())
    }

    pub fn profile_repository<'a>(&'a self, profile: &'a Profile) -> &'a str {
        match (&self.github, &self.builtin) {
            (Some(github), None) => profile.repository.as_deref().unwrap_or(&github.repository),
            (None, Some(builtin)) => &builtin.repository,
            _ => unreachable!("configuration validates exactly one collaboration backend"),
        }
    }

    pub fn profiles_for_repository<'a>(&'a self, repository: &str, tag: &str) -> Vec<&'a Profile> {
        self.profiles
            .iter()
            .filter(|profile| {
                self.profile_repository(profile) == repository && profile.has_tag(tag)
            })
            .collect()
    }

    pub fn issue_profile_for_repository(&self, repository: &str) -> Result<&Profile, ConfigError> {
        self.profiles_for_repository(repository, "issue").into_iter().next().ok_or_else(|| {
            ConfigError::Invalid(format!("repository {repository:?} has no Issue Profile"))
        })
    }

    pub fn pr_profile_for_repository(&self, repository: &str) -> Result<&Profile, ConfigError> {
        let candidates = self.profiles_for_repository(repository, "pr");
        if let [profile] = candidates.as_slice() {
            return Ok(profile);
        }
        candidates
            .iter()
            .find(|profile| profile.id == self.profile_selection.default_pr_profile)
            .copied()
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "repository {repository:?} does not have an unambiguous PR Profile"
                ))
            })
    }

    pub fn github(&self) -> Result<&GitHubConfig, ConfigError> {
        self.github.as_ref().ok_or_else(|| {
            ConfigError::Invalid(
                "this command requires the GitHub backend; this Harness connects to the built-in collaboration platform"
                    .into(),
            )
        })
    }

    pub fn profile(&self, id: &str) -> Result<&Profile, ConfigError> {
        self.profiles
            .iter()
            .find(|profile| profile.id == id)
            .ok_or_else(|| ConfigError::Invalid(format!("unknown profile {id:?}")))
    }

    pub fn summary(&self) -> ConfigSummary<'_> {
        ConfigSummary {
            schema_version: self.schema_version,
            upstream: if self.is_builtin() { "builtin" } else { "github" },
            instance_key: &self.instance.key,
            repository: self.repository(),
            repositories: self.repositories(),
            runtime_root: self.runtime.root(),
            database: self.runtime.database(),
            profile_ids: self.profiles.iter().map(|profile| profile.id.as_str()).collect(),
            default_pr_profile: &self.profile_selection.default_pr_profile,
            trace_sample_ratio: self.telemetry.sample_ratio,
            incident_mode: self.telemetry.incident_mode,
            effective_trace_sample_ratio: self.telemetry.effective_sample_ratio(),
        }
    }

    pub(crate) fn profile_materials(
        &self,
        profile: &Profile,
    ) -> Result<crate::profile_materials::EffectiveMaterials, ConfigError> {
        crate::profile_materials::collect(self, profile).map_err(|error| {
            ConfigError::Invalid(format!("profile {:?} materials: {error:#}", profile.id))
        })
    }

    /// Load the GitHub webhook secret from `github.webhook_secret_file` if set,
    /// otherwise from `github.webhook_secret_environment`.
    pub fn webhook_secret(&self) -> Result<String, ConfigError> {
        if let Some(path) = &self.github()?.webhook_secret_file {
            return load_webhook_secret_file(path);
        }
        if let Some(env) = &self.github()?.webhook_secret_environment {
            return std::env::var(env).map_err(|_| {
                ConfigError::Invalid(format!(
                    "environment variable {env:?} for github.webhook_secret_environment is not set"
                ))
            });
        }
        Err(ConfigError::Invalid(
            "one of github.webhook_secret_environment or github.webhook_secret_file must be set"
                .into(),
        ))
    }

    pub fn runtime_for(&self, profile: &Profile) -> Result<&RuntimeEntry, ConfigError> {
        self.runtimes
            .iter()
            .find(|runtime| runtime.adapter_type == profile.adapter_type)
            .ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "profile {:?} references unknown adapter_type {:?}",
                    profile.id, profile.adapter_type
                ))
            })
    }

    pub fn llm_provider_for(&self, provider_id: &str) -> Result<&LlmProvider, ConfigError> {
        self.llm_providers.iter().find(|provider| provider.id == provider_id).ok_or_else(|| {
            ConfigError::Invalid(format!("unknown llm_providers entry {provider_id:?}"))
        })
    }

    /// Provider settings for the default PR Profile, used by operator probes.
    pub fn default_provider_config(&self) -> Result<ProviderConfig, ConfigError> {
        self.provider_config_for_profile(self.profile(&self.profile_selection.default_pr_profile)?)
    }

    pub(crate) fn provider_config_for_profile(
        &self,
        profile: &Profile,
    ) -> Result<ProviderConfig, ConfigError> {
        self.provider_config_for_runtime(self.runtime_for(profile)?, profile)
    }

    fn provider_config_for_runtime(
        &self,
        runtime: &RuntimeEntry,
        profile: &Profile,
    ) -> Result<ProviderConfig, ConfigError> {
        if runtime.adapter_type == "codex" {
            let home = runtime.home.clone().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "runtime {:?} requires home for codex adapter",
                    runtime.adapter_type
                ))
            })?;
            let stable = runtime.stable_schema_sha256.clone().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "runtime {:?} requires stable_schema_sha256",
                    runtime.adapter_type
                ))
            })?;
            let experimental = runtime.experimental_schema_sha256.clone().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "runtime {:?} requires experimental_schema_sha256",
                    runtime.adapter_type
                ))
            })?;
            return Ok(ProviderConfig {
                codex: Some(CodexConfig {
                    braid_config_path: Some(self.loaded_from.clone()),
                    executable: runtime.executable.clone(),
                    home,
                    version: runtime.version.clone(),
                    stable_schema_sha256: stable,
                    experimental_schema_sha256: experimental,
                }),
                pi: None,
            });
        }

        if runtime.adapter_type == "pi" {
            // Pi needs an explicit LLM provider entry for its API key and model route.
            let provider_id = profile.provider.as_deref().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "profile {:?} requires provider for the Pi adapter",
                    profile.id
                ))
            })?;
            let llm = self.llm_provider_for(provider_id)?;
            return Ok(ProviderConfig {
                codex: None,
                pi: Some(PiConfig {
                    native_runtime: runtime.native_runtime.clone(),
                    braid_config_path: Some(self.loaded_from.clone()),
                    executable: runtime.executable.clone(),
                    provider: Some(llm.id.clone()),
                    model: profile.model.clone(),
                    credential: llm.resolved_credential(),
                    thinking: profile.reasoning.clone(),
                    home: runtime.home.clone(),
                    profile_material_digest: self.profile_materials(profile)?.digest,
                }),
            });
        }

        Err(ConfigError::Invalid(format!("unsupported adapter_type {:?}", runtime.adapter_type)))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION && self.schema_version != 2 {
            return Err(ConfigError::Schema {
                found: self.schema_version,
                supported: CONFIG_SCHEMA_VERSION,
            });
        }
        validate_instance_key(&self.instance.key)
            .map_err(|message| ConfigError::Invalid(format!("instance.key: {message}")))?;

        if self.local.is_some() {
            return Err(ConfigError::Invalid("[local] combined platform and Harness state is no longer supported; stop the old runtime, import its objects with braid platform import, then replace [local] with [builtin] repository and endpoint; see docs/user-manual/local.md".into()));
        }
        if self.github.is_some() == self.builtin.is_some() {
            return Err(ConfigError::Invalid(
                "configure exactly one upstream: [github] or [builtin]".into(),
            ));
        }
        for (name, path) in [
            ("runtime.root", self.runtime.root()),
            ("runtime.database", self.runtime.database()),
            ("runtime.backups", self.runtime.backups()),
            ("tools.git", self.tools.git.as_path()),
        ] {
            require_absolute(name, path)?;
        }
        if !self.runtime.database().starts_with(self.runtime.root())
            || !self.runtime.backups().starts_with(self.runtime.root())
        {
            return Err(ConfigError::Invalid(
                "runtime database and backups must be inside runtime.root".into(),
            ));
        }
        if let Some(builtin) = &self.builtin {
            validate_token("builtin.repository", &builtin.repository)?;
            let endpoint = &builtin.endpoint;
            let loopback = match endpoint.host() {
                Some(url::Host::Domain("localhost")) => true,
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            };
            if endpoint.scheme() != "http"
                || !loopback
                || !endpoint.username().is_empty()
                || endpoint.password().is_some()
                || endpoint.query().is_some()
                || endpoint.fragment().is_some()
                || endpoint.path() != "/"
            {
                return Err(ConfigError::Invalid("builtin.endpoint must be a loopback HTTP origin without credentials, query, fragment or path".into()));
            }
            for profile in &self.profiles {
                if profile.workspace.is_none() {
                    return Err(ConfigError::Invalid(format!(
                        "profile {:?} requires workspace for the built-in platform's Git source",
                        profile.id
                    )));
                }
            }
        }
        if let Some(github) = &self.github {
            require_absolute("github.private_key_file", &github.private_key_file)?;
            require_absolute("tools.gh", &self.tools.gh)?;
            require_absolute("tools.wrangler", &self.tools.wrangler)?;
            if let Some(path) = &github.webhook_secret_file {
                require_absolute("github.webhook_secret_file", path)?;
            }
            if github.app_id == 0 {
                return Err(ConfigError::Invalid("github.app_id must be positive".into()));
            }
            validate_repository(&github.repository)?;
            let mut repository_names = HashSet::new();
            repository_names.insert(github.repository.as_str());
            for repository in &github.additional_repositories {
                validate_repository(&repository.name)?;
                if !repository_names.insert(repository.name.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate GitHub repository {:?}",
                        repository.name
                    )));
                }
            }
            validate_token("github.handle", &github.handle)?;
            if github.webhook_secret_environment.is_none() && github.webhook_secret_file.is_none() {
                return Err(ConfigError::Invalid("one of github.webhook_secret_environment or github.webhook_secret_file must be set".into()));
            }
            if let Some(env) = &github.webhook_secret_environment {
                validate_token("github.webhook_secret_environment", env)?;
            }
        }
        if self.scheduler.quiet_seconds == 0
            || self.scheduler.event_threshold == 0
            || self.scheduler.reconciliation_seconds == 0
        {
            return Err(ConfigError::Invalid(
                "scheduler durations and event threshold must be positive".into(),
            ));
        }
        for (name, address) in
            [("server.ingress", self.server.ingress), ("server.health", self.server.health)]
        {
            if !address.ip().is_loopback() {
                return Err(ConfigError::Invalid(format!("{name} must use a loopback address")));
            }
        }
        if self.server.ingress == self.server.health {
            return Err(ConfigError::Invalid(
                "server.ingress and server.health must differ".into(),
            ));
        }
        self.telemetry.validate()?;

        if self.runtimes.is_empty() {
            return Err(ConfigError::Invalid("at least one runtime is required".into()));
        }
        let mut runtime_types = HashSet::new();
        for runtime in &self.runtimes {
            validate_token("runtime.adapter_type", &runtime.adapter_type)?;
            require_absolute(
                &format!("runtime.{}.executable", runtime.adapter_type),
                &runtime.executable,
            )?;
            if let Some(manifest) = &runtime.native_runtime {
                require_absolute("runtime.native_runtime", manifest)?;
                if runtime.adapter_type != "pi" {
                    return Err(ConfigError::Invalid(
                        "native_runtime is only supported by Pi".into(),
                    ));
                }
            }
            if !runtime_types.insert(runtime.adapter_type.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate runtime adapter_type {:?}",
                    runtime.adapter_type
                )));
            }
            if runtime.adapter_type == "codex" {
                if runtime.home.is_none() {
                    return Err(ConfigError::Invalid(format!(
                        "runtime {:?} requires home",
                        runtime.adapter_type
                    )));
                }
                if let Some(home) = &runtime.home {
                    require_absolute(&format!("runtime.{}.home", runtime.adapter_type), home)?;
                }
                if runtime.stable_schema_sha256.is_none()
                    || runtime.experimental_schema_sha256.is_none()
                {
                    return Err(ConfigError::Invalid(format!(
                        "runtime {:?} requires stable_schema_sha256 and experimental_schema_sha256",
                        runtime.adapter_type
                    )));
                }
            }
            if runtime.adapter_type == "pi"
                && let Some(home) = &runtime.home
            {
                require_absolute(&format!("runtime.{}.home", runtime.adapter_type), home)?;
            }
        }

        let mut provider_ids = HashSet::new();
        for provider in &self.llm_providers {
            validate_token("llm_providers.id", &provider.id)?;
            if let Some(credential) = &provider.credential {
                validate_credential(
                    &format!("llm_providers.{}.credential", provider.id),
                    credential,
                )?;
            }
            if let Some(api_key_file) = &provider.api_key_file {
                require_absolute(
                    &format!("llm_providers.{}.api_key_file", provider.id),
                    api_key_file,
                )?;
            }
            if self.schema_version == 2 {
                if let Some(protocol) = &provider.protocol {
                    validate_token("llm_providers.protocol", protocol)?;
                }
            } else {
                if provider.api_key_file.is_some() {
                    return Err(ConfigError::Invalid(format!(
                        "llm_providers {:?} uses legacy api_key_file; schema 3 uses credential = {{ kind = \"api_key\", file = ... }}",
                        provider.id
                    )));
                }
                if provider.credential.is_none() {
                    return Err(ConfigError::Invalid(format!(
                        "llm_providers {:?} requires an explicit credential; use kind = \"none\" for a keyless route",
                        provider.id
                    )));
                }
                if provider.protocol.is_some()
                    || !provider.allowances.is_empty()
                    || provider.models.iter().any(|model| {
                        model.input_cost.is_some()
                            || model.output_cost.is_some()
                            || model.cache_input_cost.is_some()
                    })
                {
                    return Err(ConfigError::Invalid(format!(
                        "llm_providers {:?} contains schema-2 protocol/cost metadata; schema 3 keeps only id, credential, and base_url/api/models metadata",
                        provider.id
                    )));
                }
                if let Some(base_url) = &provider.base_url {
                    validate_provider_base_url(
                        &format!("llm_providers.{}.base_url", provider.id),
                        base_url,
                    )?;
                }
            }
            if !provider_ids.insert(provider.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate llm_providers id {:?}",
                    provider.id
                )));
            }
            let mut model_ids = HashSet::new();
            for model in &provider.models {
                validate_token(&format!("llm_providers.{}.models.id", provider.id), &model.id)?;
                if !model_ids.insert(model.id.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate model {:?} in llm_providers {:?}",
                        model.id, provider.id
                    )));
                }
                if model.context_window == Some(0) || model.max_tokens == Some(0) {
                    return Err(ConfigError::Invalid(format!(
                        "llm_providers.{} model {:?} requires positive context_window and max_tokens",
                        provider.id, model.id
                    )));
                }
            }
        }

        if self.profiles.is_empty() {
            return Err(ConfigError::Invalid("at least one profile is required".into()));
        }
        let mut ids = HashSet::new();
        let mut repository_workspaces = HashMap::new();
        for profile in &self.profiles {
            let repository = self.profile_repository(profile);
            if let Some(workspace) = &profile.workspace
                && let Some(existing) =
                    repository_workspaces.insert(repository, workspace.as_path())
                && existing != workspace
            {
                return Err(ConfigError::Invalid(format!(
                    "profiles for repository {repository:?} must share one source checkout; {} and {} differ",
                    existing.display(),
                    workspace.display()
                )));
            }
            profile.validate(self)?;
            if !ids.insert(profile.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "profile id {:?} is duplicated",
                    profile.id
                )));
            }
        }
        if self.github.is_some() {
            for repository in self.github_repositories() {
                if self.profiles_for_repository(repository, "issue").is_empty()
                    && self.profiles_for_repository(repository, "pr").is_empty()
                {
                    return Err(ConfigError::Invalid(format!(
                        "GitHub repository {repository:?} has no Issue or PR Profile"
                    )));
                }
            }
        }
        let default_pr = self.profile(&self.profile_selection.default_pr_profile)?;
        if !default_pr.has_tag("pr") {
            return Err(ConfigError::Invalid(
                "profile_selection.default_pr_profile must reference a profile tagged pr".into(),
            ));
        }

        // Temporary MVP bridge: all profiles share one adapter_type.
        let first_adapter = default_pr.adapter_type.clone();
        if self.profiles.iter().any(|profile| profile.adapter_type != first_adapter) {
            return Err(ConfigError::Invalid(
                "this release requires all profiles to use the same adapter_type".into(),
            ));
        }

        Ok(())
    }
}

impl TelemetryConfig {
    pub fn effective_sample_ratio(&self) -> f64 {
        if self.incident_mode { 1.0 } else { self.sample_ratio }
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if !(0.0..=1.0).contains(&self.sample_ratio) {
            return Err(ConfigError::Invalid(
                "telemetry.sample_ratio must be between 0 and 1".into(),
            ));
        }
        if !matches!(self.endpoint.scheme(), "http" | "https") {
            return Err(ConfigError::Invalid("telemetry.endpoint must use http or https".into()));
        }
        if self.endpoint.host_str().is_none() || self.export_timeout_seconds == 0 {
            return Err(ConfigError::Invalid(
                "telemetry endpoint host and positive export timeout are required".into(),
            ));
        }
        validate_token("telemetry.service_name", &self.service_name)
    }
}

impl Profile {
    fn validate_pi_provider_route(&self, config: &Config) -> Result<(), ConfigError> {
        if self.adapter_type != "pi" {
            return Ok(());
        }
        let provider_id = self.provider.as_deref().expect("Pi provider checked above");
        let llm = config.llm_provider_for(provider_id)?;
        if config.schema_version == 2 {
            return Ok(());
        }
        if llm.base_url.is_none() {
            return Err(ConfigError::Invalid(format!(
                "llm_providers {:?} requires base_url for Pi models.json derivation",
                llm.id
            )));
        }
        if let Some(model) = &self.model
            && !llm.models.iter().any(|declared| declared.id == *model)
        {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} model {:?} is not declared in llm_providers {:?} models",
                self.id, model, llm.id
            )));
        }
        Ok(())
    }

    fn validate_repository_scope(&self, config: &Config) -> Result<(), ConfigError> {
        let Some(github) = &config.github else {
            if self.repository.is_some() {
                return Err(ConfigError::Invalid(format!(
                    "profile {:?} repository is supported only by the GitHub backend",
                    self.id
                )));
            }
            return Ok(());
        };
        let configured = config.github_repositories();
        match &self.repository {
            Some(repository) => {
                validate_repository(repository)?;
                if !configured.contains(&repository.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "profile {:?} references unconfigured GitHub repository {:?}",
                        self.id, repository
                    )));
                }
            }
            None => {
                if !github.additional_repositories.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "profile {:?} requires an explicit repository in a multi-repository config",
                        self.id
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|candidate| candidate == tag)
    }

    fn validate(&self, config: &Config) -> Result<(), ConfigError> {
        validate_token("profile.id", &self.id)?;
        if self.display_name.trim().is_empty() || self.user_instructions.trim().is_empty() {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} requires display_name and user_instructions",
                self.id
            )));
        }
        validate_token("profile.adapter_type", &self.adapter_type)?;
        if self.adapter_version.trim().is_empty() {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} adapter_version must be non-empty",
                self.id
            )));
        }
        if self.adapter_type == "pi" {
            let provider = self.provider.as_deref().ok_or_else(|| {
                ConfigError::Invalid(format!(
                    "profile {:?} requires provider for the Pi adapter",
                    self.id
                ))
            })?;
            validate_token("profile.provider", provider)?;
            if self.model.is_none() && config.schema_version != 2 {
                return Err(ConfigError::Invalid(format!(
                    "profile {:?} requires an explicit model for the Pi adapter",
                    self.id
                )));
            }
        } else if self.provider.is_some() && config.schema_version != 2 {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} provider is supported only by the Pi adapter",
                self.id
            )));
        }
        self.validate_repository_scope(config)?;
        if let Some(workspace) = &self.workspace {
            require_absolute(&format!("profile {:?}.workspace", self.id), workspace)?;
        }
        if self.adapter_type != "pi" && (!self.skills.is_empty() || self.native_template.is_some())
        {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} skills and native_template are supported only by the Pi adapter",
                self.id
            )));
        }
        for skill in &self.skills {
            require_absolute(&format!("profile {:?}.skills", self.id), skill)?;
        }
        if let Some(template) = &self.native_template {
            require_absolute(&format!("profile {:?}.native_template", self.id), template)?;
        }
        if self.tags.is_empty() || (!self.has_tag("issue") && !self.has_tag("pr")) {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} must have issue or pr capability tag",
                self.id
            )));
        }
        if self.has_tag("implementation") && !self.has_tag("pr") {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} implementation tag requires pr",
                self.id
            )));
        }
        let unique_tags: HashSet<&str> = self.tags.iter().map(String::as_str).collect();
        if unique_tags.len() != self.tags.len() {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} contains duplicate tags",
                self.id
            )));
        }
        if self.status_surfaces.iter().any(|surface| surface != "issue" && surface != "pr") {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} status_surfaces accepts only issue or pr",
                self.id
            )));
        }
        if !(0.0..1.0).contains(&self.github_context_soft_ratio)
            || self.github_context_hard_bytes == 0
        {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} requires github_context_soft_ratio in (0,1) and positive github_context_hard_bytes",
                self.id
            )));
        }

        // Resolve references.
        let runtime = config.runtime_for(self)?;
        if runtime.version != self.adapter_version {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} adapter_version {:?} does not match runtime {:?} version {:?}",
                self.id, self.adapter_version, runtime.adapter_type, runtime.version
            )));
        }
        self.validate_pi_provider_route(config)?;
        // The profile's model is a request to the provider, and the provider
        // is the authority on which models it accepts. The llm_providers
        // catalog only carries cost metadata, so an uncatalogued model is
        // not a config error: Codex must be able to answer with a real
        // turn failure for a model it does not support.
        config.profile_materials(self)?;
        Ok(())
    }
}

fn validate_provider_base_url(name: &str, value: &str) -> Result<(), ConfigError> {
    let url = url::Url::parse(value)
        .map_err(|error| ConfigError::Invalid(format!("{name} is not a valid URL: {error}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ConfigError::Invalid(format!("{name} must use http or https")));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ConfigError::Invalid(format!("{name} must not carry credentials")));
    }
    if url.query_pairs().any(|(key, _)| sensitive_query_name(&key)) {
        return Err(ConfigError::Invalid(format!(
            "{name} must not carry credential query parameters"
        )));
    }
    Ok(())
}

pub(crate) fn sensitive_query_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "api-key",
        "api_key",
        "apikey",
        "auth",
        "authorization",
        "token",
        "secret",
        "password",
        "credential",
        "signature",
        "key",
        "access-token",
        "access_token",
    ]
    .iter()
    .any(|exact| name == *exact)
}

fn validate_credential(name: &str, credential: &ProviderCredential) -> Result<(), ConfigError> {
    match credential.kind {
        CredentialKind::None => {
            if credential.file.is_some() {
                return Err(ConfigError::Invalid(format!(
                    "{name}: kind \"none\" must not reference a file"
                )));
            }
        }
        CredentialKind::ApiKey | CredentialKind::OAuth => match &credential.file {
            Some(file) => require_absolute(name, file)?,
            None => {
                return Err(ConfigError::Invalid(format!(
                    "{name}: kind {:?} requires a file",
                    credential.kind
                )));
            }
        },
    }
    Ok(())
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

fn require_absolute(name: &str, path: &Path) -> Result<(), ConfigError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!("{name} must be an absolute path")))
    }
}

fn validate_repository(repository: &str) -> Result<(), ConfigError> {
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty() || name.is_empty() || parts.next().is_some() {
        return Err(ConfigError::Invalid("github.repository must be owner/repository".into()));
    }
    validate_token("github.repository owner", owner)?;
    validate_token("github.repository name", name)
}

pub(crate) fn validate_token(name: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() || value.chars().any(char::is_whitespace) {
        Err(ConfigError::Invalid(format!("{name} must be a non-empty token")))
    } else {
        Ok(())
    }
}

const fn default_log_format() -> LogFormat {
    LogFormat::Text
}

/// Attribution markers identifying Braid-authored Agent text, derived
/// from the configured Profiles. Used by ingress/reconcile to classify
/// authorship; a pure derivation with no state of its own.
pub fn agent_attributions(config: &Config) -> Vec<String> {
    let mut attributions = Vec::new();
    for profile in &config.profiles {
        if profile.has_tag("issue") {
            attributions
                .push(format!("> **Braid Agent · {}**\n> Issue Agent", profile.display_name));
        }
        if profile.has_tag("pr") {
            attributions.push(format!(
                "> **Braid Agent · {}**\n> PR Implementation Agent",
                profile.display_name
            ));
        }
    }
    attributions
}
