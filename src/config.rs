use std::{
    collections::HashSet,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("cannot read config {path}: {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("cannot parse TOML config {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
    #[error("unsupported config schema {found}; this binary supports {supported}")]
    Schema { found: u32, supported: u32 },
    #[error("invalid config: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub runtime: RuntimeConfig,
    pub github: GitHubConfig,
    pub scheduler: SchedulerConfig,
    /// Adapter connectivity, keyed by `adapter_type`. One entry per `adapter_type`
    /// per worker; the profile's `adapter_type` locates the runtime entry.
    pub runtimes: Vec<RuntimeEntry>,
    /// LLM provider catalogue. Profiles reference an entry by id; enforcement is
    /// outside the scope of this pass.
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
    /// Defaults to the directory containing the loaded config file.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Defaults to `<root>/braid.db`.
    #[serde(default)]
    pub database: Option<PathBuf>,
    /// Defaults to `<root>/backups`.
    #[serde(default)]
    pub backups: Option<PathBuf>,
    #[serde(default)]
    pub auto_migrate: bool,
}

impl RuntimeConfig {
    /// Fill omitted/relative paths using `base` (the directory containing the
    /// loaded config file) and normalize relative paths to absolute paths.
    pub fn resolve(&mut self, base: &Path) {
        if self.root.is_none() {
            self.root = Some(base.to_path_buf());
        }
        let root = self.root.clone().expect("root resolved above");
        let root = resolve_path(base, &root);
        if self.database.is_none() {
            self.database = Some(root.join("braid.db"));
        }
        if self.backups.is_none() {
            self.backups = Some(root.join("backups"));
        }
        self.root = Some(root);
        self.database =
            Some(resolve_path(base, self.database.as_ref().expect("database resolved")));
        self.backups = Some(resolve_path(base, self.backups.as_ref().expect("backups resolved")));
    }

    fn require_resolved(&self) {
        assert!(
            self.root.is_some() && self.database.is_some() && self.backups.is_some(),
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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEntry {
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

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmProvider {
    pub id: String,
    pub protocol: String,
    #[serde(default)]
    pub api_key_environment: Option<String>,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
    #[serde(default)]
    pub models: Vec<LlmModel>,
    #[serde(default)]
    pub allowances: Vec<LlmAllowance>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LlmModel {
    pub model_id: String,
    #[serde(default)]
    pub input_cost: f64,
    #[serde(default)]
    pub output_cost: f64,
    #[serde(default)]
    pub cache_input_cost: f64,
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
pub struct GitHubConfig {
    pub app_id: u64,
    pub repository: String,
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
    pub executable: PathBuf,
    pub home: PathBuf,
    pub version: String,
    pub stable_schema_sha256: String,
    pub experimental_schema_sha256: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PiConfig {
    pub executable: PathBuf,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub api_key_environment: Option<String>,
    #[serde(default)]
    pub api_key_file: Option<PathBuf>,
    pub thinking: Option<String>,
    pub home: Option<PathBuf>,
}

impl PiConfig {
    /// Load the provider API key from `api_key_file` if set, otherwise from
    /// `api_key_environment`.
    pub fn api_key(&self) -> Result<String, ConfigError> {
        if let Some(path) = &self.api_key_file {
            return Ok(load_secret_file(path)?.provider_api_key);
        }
        if let Some(env) = &self.api_key_environment {
            return std::env::var(env).map_err(|_| {
                ConfigError::Invalid(format!(
                    "environment variable {env:?} for provider.pi.api_key_environment is not set"
                ))
            });
        }
        Err(ConfigError::Invalid(
            "one of provider.pi.api_key_environment or provider.pi.api_key_file must be set".into(),
        ))
    }
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
    pub provider: String,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    pub user_instructions: String,
    pub workspace: PathBuf,
    pub github_actor_node_id: Option<String>,
    pub status_surfaces: Vec<String>,
    pub github_context_soft_ratio: f64,
    pub github_context_hard_bytes: usize,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSelection {
    pub default_pr_profile: String,
}

#[derive(Debug, Serialize)]
pub struct ConfigSummary<'a> {
    pub schema_version: u32,
    pub repository: &'a str,
    pub runtime_root: &'a Path,
    pub database: &'a Path,
    pub profile_ids: Vec<&'a str>,
    pub default_pr_profile: &'a str,
    pub trace_sample_ratio: f64,
    pub incident_mode: bool,
    pub effective_trace_sample_ratio: f64,
}

/// Secrets file shape referenced by `github.webhook_secret_file` and runtime
/// adapter `api_key_file`. A single file can hold both secrets for a worker,
/// keeping user-scope runtime directories out of the secret lookup path.
#[derive(Debug, Deserialize)]
pub struct SecretsFile {
    pub webhook_secret: String,
    pub provider_api_key: String,
}

fn load_secret_file(path: &Path) -> Result<SecretsFile, ConfigError> {
    let text = fs::read_to_string(path)
        .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
    toml::from_str(&text).map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path)
            .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
        let mut config: Self = toml::from_str(&text)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })?;
        let base = path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let base = if base.is_absolute() {
            base
        } else {
            std::env::current_dir().map(|cwd| cwd.join(&base)).unwrap_or(base)
        };
        config.runtime.resolve(&base);
        config.validate()?;
        Ok(config)
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
            repository: &self.github.repository,
            runtime_root: self.runtime.root(),
            database: self.runtime.database(),
            profile_ids: self.profiles.iter().map(|profile| profile.id.as_str()).collect(),
            default_pr_profile: &self.profile_selection.default_pr_profile,
            trace_sample_ratio: self.telemetry.sample_ratio,
            incident_mode: self.telemetry.incident_mode,
            effective_trace_sample_ratio: self.telemetry.effective_sample_ratio(),
        }
    }

    /// Load the GitHub webhook secret from `github.webhook_secret_file` if set,
    /// otherwise from `github.webhook_secret_environment`.
    pub fn webhook_secret(&self) -> Result<String, ConfigError> {
        if let Some(path) = &self.github.webhook_secret_file {
            return Ok(load_secret_file(path)?.webhook_secret);
        }
        if let Some(env) = &self.github.webhook_secret_environment {
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

    pub fn llm_provider_for(&self, profile: &Profile) -> Result<&LlmProvider, ConfigError> {
        self.llm_providers.iter().find(|provider| provider.id == profile.provider).ok_or_else(
            || {
                ConfigError::Invalid(format!(
                    "profile {:?} references unknown provider {:?}",
                    profile.id, profile.provider
                ))
            },
        )
    }

    /// Temporary MVP bridge: all profiles must share the same `adapter_type` so
    /// that the existing single-provider runtime code can connect once. This
    /// restriction is lifted when the `AgentSession` trait refactor lands.
    pub fn provider_config(&self) -> Result<ProviderConfig, ConfigError> {
        let default_profile = self.profile(&self.profile_selection.default_pr_profile)?;
        let runtime = self.runtime_for(default_profile)?;
        self.provider_config_for_runtime(runtime)
    }

    fn provider_config_for_runtime(
        &self,
        runtime: &RuntimeEntry,
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
            // Pi needs an LLM provider entry for its API key and model info.
            let default_profile = self.profile(&self.profile_selection.default_pr_profile)?;
            let llm = self.llm_provider_for(default_profile)?;
            return Ok(ProviderConfig {
                codex: None,
                pi: Some(PiConfig {
                    executable: runtime.executable.clone(),
                    provider: Some(llm.id.clone()),
                    model: default_profile.model.clone(),
                    api_key_environment: llm.api_key_environment.clone(),
                    api_key_file: llm.api_key_file.clone(),
                    thinking: default_profile.reasoning.clone(),
                    home: runtime.home.clone(),
                }),
            });
        }

        Err(ConfigError::Invalid(format!("unsupported adapter_type {:?}", runtime.adapter_type)))
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(ConfigError::Schema {
                found: self.schema_version,
                supported: CONFIG_SCHEMA_VERSION,
            });
        }

        for (name, path) in [
            ("runtime.root", self.runtime.root()),
            ("runtime.database", self.runtime.database()),
            ("runtime.backups", self.runtime.backups()),
            ("github.private_key_file", &self.github.private_key_file),
            ("tools.git", &self.tools.git),
            ("tools.gh", &self.tools.gh),
            ("tools.wrangler", &self.tools.wrangler),
        ]
        .into_iter()
        .chain(
            self.github
                .webhook_secret_file
                .as_ref()
                .map(|path| ("github.webhook_secret_file", path.as_path())),
        ) {
            require_absolute(name, path)?;
        }
        if !self.runtime.database().starts_with(self.runtime.root())
            || !self.runtime.backups().starts_with(self.runtime.root())
        {
            return Err(ConfigError::Invalid(
                "runtime database and backups must be inside runtime.root".into(),
            ));
        }
        if self.github.app_id == 0 {
            return Err(ConfigError::Invalid("github.app_id must be positive".into()));
        }
        validate_repository(&self.github.repository)?;
        validate_token("github.handle", &self.github.handle)?;
        if self.github.webhook_secret_environment.is_none()
            && self.github.webhook_secret_file.is_none()
        {
            return Err(ConfigError::Invalid(
                "one of github.webhook_secret_environment or github.webhook_secret_file must be set".into(),
            ));
        }
        if let Some(env) = &self.github.webhook_secret_environment {
            validate_token("github.webhook_secret_environment", env)?;
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
            validate_token("llm_providers.protocol", &provider.protocol)?;
            if provider.api_key_environment.is_none() && provider.api_key_file.is_none() {
                return Err(ConfigError::Invalid(format!(
                    "llm_providers {:?} requires api_key_environment or api_key_file",
                    provider.id
                )));
            }
            if let Some(file) = &provider.api_key_file {
                require_absolute(&format!("llm_providers.{}.api_key_file", provider.id), file)?;
            }
            if !provider_ids.insert(provider.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "duplicate llm_providers id {:?}",
                    provider.id
                )));
            }
            let mut model_ids = HashSet::new();
            for model in &provider.models {
                validate_token(
                    &format!("llm_providers.{}.model_id", provider.id),
                    &model.model_id,
                )?;
                if !model_ids.insert(model.model_id.as_str()) {
                    return Err(ConfigError::Invalid(format!(
                        "duplicate model {:?} in llm_providers {:?}",
                        model.model_id, provider.id
                    )));
                }
            }
        }

        if self.profiles.is_empty() {
            return Err(ConfigError::Invalid("at least one profile is required".into()));
        }
        let mut ids = HashSet::new();
        for profile in &self.profiles {
            profile.validate(self)?;
            if !ids.insert(profile.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "profile id {:?} is duplicated",
                    profile.id
                )));
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
        validate_token("profile.provider", &self.provider)?;
        require_absolute(&format!("profile {:?}.workspace", self.id), &self.workspace)?;
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
        let llm = config.llm_provider_for(self)?;
        if let Some(model_id) = &self.model
            && !llm.models.iter().any(|model| model.model_id == *model_id)
        {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} model {:?} not found in llm_providers {:?}",
                self.id, model_id, llm.id
            )));
        }

        Ok(())
    }
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

fn validate_token(name: &str, value: &str) -> Result<(), ConfigError> {
    if value.trim().is_empty() || value.chars().any(char::is_whitespace) {
        Err(ConfigError::Invalid(format!("{name} must be a non-empty token")))
    } else {
        Ok(())
    }
}

pub fn validate_sha256(name: &str, value: &str) -> Result<(), ConfigError> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(ConfigError::Invalid(format!("{name} must be a 64-character SHA-256")))
    }
}

const fn default_log_format() -> LogFormat {
    LogFormat::Text
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::Config;

    /// `config.example.toml` is the canonical starter template, not a loose
    /// documentation snippet. It must parse and validate against the canonical
    /// `Config` type so that it cannot drift from the real schema. Partial
    /// examples in runbooks are not required to pass this test.
    #[test]
    fn example_config_matches_schema() {
        Config::load(Path::new("config.example.toml"))
            .expect("config.example.toml must match the Config schema");
    }
}
