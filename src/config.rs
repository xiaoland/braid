use std::{
    collections::HashSet,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

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
    pub provider: ProviderConfig,
    pub tools: ToolConfig,
    pub server: ServerConfig,
    pub telemetry: TelemetryConfig,
    pub profiles: Vec<Profile>,
    pub profile_selection: ProfileSelection,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub root: PathBuf,
    pub database: PathBuf,
    pub backups: PathBuf,
    #[serde(default)]
    pub auto_migrate: bool,
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

/// Secrets file shape referenced by `github.webhook_secret_file` and
/// `provider.pi.api_key_file`. A single file can hold both secrets for an
/// owner, keeping user-scope runtime directories out of the secret lookup path.
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
        let config: Self = toml::from_str(&text)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })?;
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
            runtime_root: &self.runtime.root,
            database: &self.runtime.database,
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

    /// Load the Pi provider API key from `provider.pi.api_key_file` if set,
    /// otherwise from `provider.pi.api_key_environment`.
    pub fn pi_api_key(&self) -> Result<String, ConfigError> {
        let pi = self.provider.pi.as_ref().ok_or_else(|| {
            ConfigError::Invalid("provider.pi must be configured to read its API key".into())
        })?;
        if let Some(path) = &pi.api_key_file {
            return Ok(load_secret_file(path)?.provider_api_key);
        }
        if let Some(env) = &pi.api_key_environment {
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

    #[allow(clippy::too_many_lines)]
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(ConfigError::Schema {
                found: self.schema_version,
                supported: CONFIG_SCHEMA_VERSION,
            });
        }

        for (name, path) in [
            ("runtime.root", &self.runtime.root),
            ("runtime.database", &self.runtime.database),
            ("runtime.backups", &self.runtime.backups),
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
                .map(|path| ("github.webhook_secret_file", path)),
        )
        .chain(
            self.provider
                .pi
                .as_ref()
                .and_then(|pi| pi.api_key_file.as_ref())
                .map(|path| ("provider.pi.api_key_file", path)),
        ) {
            require_absolute(name, path)?;
        }
        if !self.runtime.database.starts_with(&self.runtime.root)
            || !self.runtime.backups.starts_with(&self.runtime.root)
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
        if let Some(codex) = &self.provider.codex {
            for (name, path) in [
                ("provider.codex.executable", &codex.executable),
                ("provider.codex.home", &codex.home),
            ] {
                require_absolute(name, path)?;
            }
            validate_sha256("provider.codex.stable_schema_sha256", &codex.stable_schema_sha256)?;
            validate_sha256(
                "provider.codex.experimental_schema_sha256",
                &codex.experimental_schema_sha256,
            )?;
        }
        if let Some(pi) = &self.provider.pi {
            require_absolute("provider.pi.executable", &pi.executable)?;
            if pi.api_key_environment.is_none() && pi.api_key_file.is_none() {
                return Err(ConfigError::Invalid(
                    "one of provider.pi.api_key_environment or provider.pi.api_key_file must be set".into(),
                ));
            }
            if pi.api_key_file.is_some() && pi.api_key_environment.is_none() {
                return Err(ConfigError::Invalid(
                    "provider.pi.api_key_environment must be set when provider.pi.api_key_file is used".into(),
                ));
            }
            if let Some(env) = &pi.api_key_environment {
                validate_token("provider.pi.api_key_environment", env)?;
            }
        }
        if self.provider.codex.is_none() && self.provider.pi.is_none() {
            return Err(ConfigError::Invalid(
                "at least one provider (codex or pi) must be configured".into(),
            ));
        }
        if self.profiles.is_empty() {
            return Err(ConfigError::Invalid("at least one profile is required".into()));
        }
        let mut ids = HashSet::new();
        for profile in &self.profiles {
            profile.validate()?;
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

    fn validate(&self) -> Result<(), ConfigError> {
        validate_token("profile.id", &self.id)?;
        if self.display_name.trim().is_empty() || self.user_instructions.trim().is_empty() {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} requires display_name and user_instructions",
                self.id
            )));
        }
        if self.provider != "codex" && self.provider != "pi" {
            return Err(ConfigError::Invalid(format!(
                "profile {:?} uses unsupported provider {:?}; supported: codex, pi",
                self.id, self.provider
            )));
        }
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
        Ok(())
    }
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

fn validate_sha256(name: &str, value: &str) -> Result<(), ConfigError> {
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
