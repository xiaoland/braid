use std::{
    fmt::Write as _,
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Config;

const BRAID_USER_HOME_ENV: &str = "BRAID_USER_HOME";
const BRAID_INSTANCE_ENV: &str = "BRAID_INSTANCE";
const BRAID_INSTANCE_HOME_ENV: &str = "BRAID_INSTANCE_HOME";
const DEFAULT_USER_HOME_DIR: &str = ".braid";
const REGISTRY_FILE: &str = "registry.toml";
const DEFAULT_PORT_BASE: u16 = 18_080;

/// Resolve the default Braid user root (`~/.braid`).
pub fn default_user_home() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(DEFAULT_USER_HOME_DIR))
        .context("cannot determine home directory")
}

/// Expand a leading `~` in a path to the current user's home directory.
pub fn expand_home(path: &Path) -> Result<PathBuf> {
    if let Some(s) = path.to_str()
        && (s == "~" || s.starts_with("~/"))
    {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        if s == "~" {
            return Ok(home);
        }
        return Ok(home.join(&s[2..]));
    }
    Ok(path.to_path_buf())
}

/// Validate a local instance key used for filesystem paths and registry lookup.
///
/// The rules intentionally match a subset of GitHub login rules, because the
/// default instance key is derived from the repository owner.
pub fn validate_instance_key(key: &str) -> Result<()> {
    let key = key.trim();
    if key.is_empty() {
        bail!("instance key must be non-empty");
    }
    if key.len() > 39 {
        bail!("instance key must be 39 characters or fewer");
    }
    if key.starts_with('-') || key.ends_with('-') {
        bail!("instance key must not start or end with '-'");
    }
    let mut previous_was_hyphen = false;
    for ch in key.chars() {
        match ch {
            'a'..='z' | '0'..='9' => previous_was_hyphen = false,
            '-' => {
                if previous_was_hyphen {
                    bail!("instance key must not contain consecutive '-'");
                }
                previous_was_hyphen = true;
            }
            _ => bail!("instance key must contain only lowercase ASCII letters, digits, and '-'"),
        }
    }
    Ok(())
}

/// The user-level Braid home directory: registry, optional user defaults, and
/// shared secrets. Multiple instances live under `instances/`.
#[derive(Debug, Clone)]
pub struct UserHome {
    root: PathBuf,
}

impl UserHome {
    /// Resolve the user root from CLI override, env, or default `~/.braid`.
    pub fn resolve(cli_override: Option<&Path>) -> Result<Self> {
        if let Some(path) = cli_override {
            return Self::new(expand_home(path)?);
        }
        if let Some(env) = std::env::var_os(BRAID_USER_HOME_ENV) {
            return Self::new(PathBuf::from(env));
        }
        Self::new(default_user_home()?)
    }

    #[allow(clippy::unnecessary_wraps)]
    pub fn new(root: PathBuf) -> Result<Self> {
        let root = if root.is_absolute() {
            root
        } else {
            std::env::current_dir().map(|cwd| cwd.join(&root)).unwrap_or(root)
        };
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn registry_path(&self) -> PathBuf {
        self.root.join(REGISTRY_FILE)
    }

    pub fn instances_dir(&self) -> PathBuf {
        self.root.join("instances")
    }

    pub fn secrets_dir(&self) -> PathBuf {
        self.root.join("secrets")
    }

    pub fn instance_dir(&self, key: &str) -> PathBuf {
        self.instances_dir().join(key)
    }

    /// Ensure the shared user-root directories exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        for dir in [self.instances_dir(), self.secrets_dir()] {
            fs::create_dir_all(&dir)
                .with_context(|| format!("cannot create user directory {}", dir.display()))?;
            #[cfg(unix)]
            set_dir_mode(&dir, 0o700)?;
        }
        Ok(())
    }

    /// Ensure the instance-level directories exist.
    pub fn ensure_instance_dirs(&self, key: &str) -> Result<()> {
        validate_instance_key(key)?;
        let base = self.instance_dir(key);
        for dir in [
            base.join("state"),
            base.join("state/backups"),
            base.join("state/worktrees"),
            base.join("provider"),
            base.join("logs"),
        ] {
            fs::create_dir_all(&dir)
                .with_context(|| format!("cannot create instance directory {}", dir.display()))?;
        }
        #[cfg(unix)]
        set_dir_mode(&base, 0o700)?;
        Ok(())
    }

    pub fn load_registry(&self) -> Result<Registry> {
        let path = self.registry_path();
        if !path.exists() {
            return Ok(Registry::empty());
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("cannot read registry {}", path.display()))?;
        let registry: Registry = toml::from_str(&text)
            .with_context(|| format!("cannot parse registry {}", path.display()))?;
        registry.validate()?;
        Ok(registry)
    }

    pub fn save_registry(&self, registry: &Registry) -> Result<()> {
        registry.validate()?;
        let path = self.registry_path();
        let text = toml::to_string_pretty(registry).context("cannot serialize registry")?;
        fs::write(&path, text)
            .with_context(|| format!("cannot write registry {}", path.display()))?;
        Ok(())
    }
}

/// Resolve an instance home path, treating relative registry `home` values as
/// rooted at `user_root`.
pub fn resolve_instance_home(user_root: &Path, entry: &InstanceEntry) -> PathBuf {
    if entry.home.is_absolute() { entry.home.clone() } else { user_root.join(&entry.home) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub schema_version: u32,
    #[serde(default)]
    pub default_instance: Option<String>,
    #[serde(default)]
    pub instances: Vec<InstanceEntry>,
}

impl Registry {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn empty() -> Self {
        Self { schema_version: Self::SCHEMA_VERSION, default_instance: None, instances: Vec::new() }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != Self::SCHEMA_VERSION {
            bail!(
                "unsupported registry schema {}; this binary supports {}",
                self.schema_version,
                Self::SCHEMA_VERSION
            );
        }
        let mut keys = std::collections::HashSet::new();
        let mut app_ids = std::collections::HashSet::new();
        let mut homes = std::collections::HashSet::new();
        for entry in &self.instances {
            validate_instance_key(&entry.key)?;
            if !keys.insert(entry.key.clone()) {
                bail!("duplicate instance key {:?} in registry", entry.key);
            }
            if !app_ids.insert(entry.github_app_id) {
                bail!(
                    "duplicate github_app_id {} in registry; one App maps to one instance",
                    entry.github_app_id
                );
            }
            let home = resolve_instance_home(Path::new("."), entry);
            if !homes.insert(home) {
                bail!("duplicate instance home {} in registry", entry.home.display());
            }
        }
        if let Some(default) = &self.default_instance {
            validate_instance_key(default)?;
            if self.get(default).is_none() {
                bail!("registry default_instance {default:?} is not registered");
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&InstanceEntry> {
        self.instances.iter().find(|entry| entry.key == key)
    }

    pub fn insert(&mut self, entry: InstanceEntry) -> Result<()> {
        if let Some(existing) = self.get(&entry.key) {
            bail!("instance key {:?} already exists (home {})", entry.key, existing.home.display());
        }
        if self.instances.iter().any(|e| e.github_app_id == entry.github_app_id) {
            bail!("github_app_id {} already registered to another instance", entry.github_app_id);
        }
        let home = resolve_instance_home(Path::new("."), &entry);
        if self
            .instances
            .iter()
            .map(|e| resolve_instance_home(Path::new("."), e))
            .any(|h| h == home)
        {
            bail!("instance home {} already registered to another instance", entry.home.display());
        }
        self.instances.push(entry);
        if self.default_instance.is_none() && self.instances.len() == 1 {
            self.default_instance = Some(self.instances[0].key.clone());
        }
        Ok(())
    }

    pub fn known_keys(&self) -> Vec<&str> {
        self.instances.iter().map(|entry| entry.key.as_str()).collect()
    }

    pub fn default_key(&self) -> Result<&str> {
        if let Some(key) = &self.default_instance {
            return Ok(key);
        }
        if self.instances.len() == 1 {
            return Ok(&self.instances[0].key);
        }
        let keys = self.known_keys();
        if keys.is_empty() {
            bail!("no instances registered; run `braid setup` first");
        }
        bail!(
            "no default instance selected; known instances: {}; pass --instance <KEY>",
            keys.join(", ")
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceEntry {
    pub key: String,
    #[serde(default)]
    pub home: PathBuf,
    pub github_app_id: u64,
    pub repository: String,
}

/// Resolve a config file path from the standard precedence chain.
///
/// Precedence:
/// 1. `cli_config`
/// 2. `BRAID_INSTANCE_HOME`
/// 3. `cli_instance` / `BRAID_INSTANCE`
/// 4. registry default (`default_instance`, or single instance)
pub fn resolve_config_path(
    cli_config: Option<&Path>,
    cli_instance: Option<&str>,
) -> Result<PathBuf> {
    let user = UserHome::resolve(None)?;
    resolve_config_path_with_user_home(&user, cli_config, cli_instance)
}

/// Resolve a config file path from the standard precedence chain, using a
/// caller-supplied user home for testability.
///
/// Precedence:
/// 1. `cli_config`
/// 2. `BRAID_INSTANCE_HOME`
/// 3. `cli_instance` / `BRAID_INSTANCE`
/// 4. registry default (`default_instance`, or single instance)
pub(crate) fn resolve_config_path_with_user_home(
    user: &UserHome,
    cli_config: Option<&Path>,
    cli_instance: Option<&str>,
) -> Result<PathBuf> {
    if let Some(path) = cli_config {
        return Ok(path.to_path_buf());
    }
    if let Some(env) = std::env::var_os(BRAID_INSTANCE_HOME_ENV) {
        return Ok(PathBuf::from(env).join("config.toml"));
    }
    let registry = user.load_registry()?;

    let key = if let Some(key) = cli_instance {
        key.to_owned()
    } else if let Ok(env) = std::env::var(BRAID_INSTANCE_ENV) {
        env
    } else {
        registry.default_key()?.to_owned()
    };

    let entry = registry.get(&key).with_context(|| {
        let mut message = format!("unknown instance {key:?}");
        let keys = registry.known_keys();
        if keys.is_empty() {
            message.push_str("; no instances registered; run `braid setup`");
        } else {
            write!(message, "; known: {}", keys.join(", ")).expect("writing to String cannot fail");
        }
        message
    })?;
    Ok(resolve_instance_home(user.root(), entry).join("config.toml"))
}

/// Allocate a free loopback ingress/health port pair for a new instance.
///
/// Ports already recorded in other registered instances' configs are skipped,
/// as are ports that fail a bind probe.
pub fn allocate_server_ports(
    registry: &Registry,
    user_root: &Path,
) -> Result<(SocketAddr, SocketAddr)> {
    let mut used_ports = std::collections::HashSet::new();
    for entry in &registry.instances {
        let config_path = resolve_instance_home(user_root, entry).join("config.toml");
        if let Ok(config) = Config::load(&config_path) {
            used_ports.insert(config.server.ingress.port());
            used_ports.insert(config.server.health.port());
        }
    }

    let base_port = DEFAULT_PORT_BASE;
    for candidate in (base_port..u16::MAX - 1).step_by(2) {
        if used_ports.contains(&candidate) || used_ports.contains(&(candidate + 1)) {
            continue;
        }
        let ingress = SocketAddr::from((IpAddr::from(Ipv4Addr::LOCALHOST), candidate));
        let health = SocketAddr::from((IpAddr::from(Ipv4Addr::LOCALHOST), candidate + 1));
        if TcpListener::bind(ingress).is_ok() && TcpListener::bind(health).is_ok() {
            return Ok((ingress, health));
        }
    }
    bail!("cannot find a free loopback port pair for ingress/health starting at {base_port}")
}

#[cfg(unix)]
fn set_dir_mode(dir: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut permissions = fs::metadata(dir)
        .with_context(|| format!("cannot read permissions of {}", dir.display()))?
        .permissions();
    permissions.set_mode(mode);
    fs::set_permissions(dir, permissions)
        .with_context(|| format!("cannot set permissions of {}", dir.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::config::CONFIG_SCHEMA_VERSION;

    fn temp_home() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("braid-home-test-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&dir).expect("create temp home");
        dir
    }

    #[test]
    fn expand_home_handles_tilde_forms() {
        let home = dirs::home_dir().expect("home directory");
        assert_eq!(expand_home(Path::new("~")).unwrap(), home);
        assert_eq!(expand_home(Path::new("~/x")).unwrap(), home.join("x"));
        assert_eq!(
            expand_home(Path::new("~other")).unwrap(),
            PathBuf::from("~other"),
            "only bare ~ and ~/ expand; ~name is left untouched"
        );
    }

    #[test]
    fn instance_key_validation() {
        assert!(validate_instance_key("inkcre").is_ok());
        assert!(validate_instance_key("my-org-1").is_ok());
        assert!(validate_instance_key("").is_err());
        assert!(validate_instance_key("Inkcre").is_err());
        assert!(validate_instance_key("-inkcre").is_err());
        assert!(validate_instance_key("inkcre-").is_err());
        assert!(validate_instance_key("ink--cre").is_err());
        assert!(validate_instance_key("ink/cre").is_err());
        assert!(validate_instance_key("ink cre").is_err());
    }

    #[test]
    fn registry_validates_uniqueness() {
        let mut registry = Registry::empty();
        registry
            .insert(InstanceEntry {
                key: "a".into(),
                home: PathBuf::from("instances/a"),
                github_app_id: 1,
                repository: "a/b".into(),
            })
            .unwrap();
        registry
            .insert(InstanceEntry {
                key: "b".into(),
                home: PathBuf::from("instances/b"),
                github_app_id: 2,
                repository: "b/c".into(),
            })
            .unwrap();
        assert_eq!(registry.default_instance.as_deref(), Some("a"));
        assert!(
            registry
                .insert(InstanceEntry {
                    key: "a".into(),
                    home: PathBuf::from("instances/c"),
                    github_app_id: 3,
                    repository: "c/d".into(),
                })
                .is_err()
        );
        assert!(
            registry
                .insert(InstanceEntry {
                    key: "c".into(),
                    home: PathBuf::from("instances/c"),
                    github_app_id: 2,
                    repository: "c/d".into(),
                })
                .is_err()
        );
        assert!(
            registry
                .insert(InstanceEntry {
                    key: "c".into(),
                    home: PathBuf::from("instances/b"),
                    github_app_id: 3,
                    repository: "c/d".into(),
                })
                .is_err()
        );
    }

    #[test]
    fn resolution_precedence() {
        let user_root = temp_home();
        let user = UserHome::new(user_root.clone()).unwrap();
        user.ensure_dirs().unwrap();
        let mut registry = Registry::empty();
        let instance_dir = user.instance_dir("inkcre");
        fs::create_dir_all(instance_dir.join("state")).unwrap();
        let config_path = instance_dir.join("config.toml");
        fs::write(
            &config_path,
            format!(
                "schema_version = {CONFIG_SCHEMA_VERSION}\n[instance]\nkey = \"inkcre\"\n[runtime]\nroot = \"{}\"\n",
                instance_dir.join("state").display()
            ),
        )
        .unwrap();
        registry
            .insert(InstanceEntry {
                key: "inkcre".into(),
                home: PathBuf::from("instances/inkcre"),
                github_app_id: 1,
                repository: "inkcre/braid".into(),
            })
            .unwrap();
        user.save_registry(&registry).unwrap();

        // 1. cli_config wins over --instance.
        assert_eq!(
            resolve_config_path_with_user_home(
                &user,
                Some(Path::new("/custom/config.toml")),
                Some("other")
            )
            .unwrap(),
            PathBuf::from("/custom/config.toml")
        );

        // 2. explicit --instance wins over registry default.
        assert_eq!(
            resolve_config_path_with_user_home(&user, None, Some("inkcre")).unwrap(),
            user_root.join("instances/inkcre/config.toml")
        );

        // 3. registry default when no instance selected.
        assert_eq!(
            resolve_config_path_with_user_home(&user, None, None).unwrap(),
            user_root.join("instances/inkcre/config.toml")
        );
    }

    #[test]
    fn port_allocation_skips_registry_ports() {
        let user_root = temp_home();
        let instance_dir = user_root.join("instances/inkcre");
        fs::create_dir_all(instance_dir.join("state")).unwrap();
        let config_text = format!(
            r#"schema_version = {CONFIG_SCHEMA_VERSION}
[instance]
key = "inkcre"
[runtime]
root = "{}"
[github]
app_id = 1
repository = "inkcre/braid"
handle = "braid"
api_version = "2022-11-28"
private_key_file = "/tmp/fake.pem"
webhook_secret_environment = "BRAID_SECRET"
projects_v2_enabled = false
[scheduler]
quiet_seconds = 1
event_threshold = 1
reconciliation_seconds = 1
[[runtimes]]
adapter_type = "pi"
version = "1"
executable = "/tmp/pi"
[[llm_providers]]
id = "deepseek"
protocol = "openai-compatible"
api_key_environment = "DEEPSEEK_API_KEY"
[[llm_providers.models]]
model_id = "deepseek-chat"
[tools]
git = "/usr/bin/git"
gh = "/usr/bin/gh"
wrangler = "/usr/bin/wrangler"
[server]
ingress = "127.0.0.1:18080"
health = "127.0.0.1:18081"
[telemetry]
endpoint = "http://127.0.0.1:4318"
sample_ratio = 0.1
export_timeout_seconds = 5
service_name = "braid"
[[profiles]]
id = "default"
display_name = "Braid"
tags = ["issue", "pr"]
adapter_type = "pi"
adapter_version = "1"
provider = "deepseek"
user_instructions = "x"
workspace = "/tmp/w"
status_surfaces = ["issue", "pr"]
github_context_soft_ratio = 0.5
github_context_hard_bytes = 1000
[profile_selection]
default_pr_profile = "default"
"#,
            instance_dir.join("state").display()
        );
        fs::write(instance_dir.join("config.toml"), config_text).unwrap();

        let mut registry = Registry::empty();
        registry
            .insert(InstanceEntry {
                key: "inkcre".into(),
                home: PathBuf::from("instances/inkcre"),
                github_app_id: 1,
                repository: "inkcre/braid".into(),
            })
            .unwrap();

        let (ingress, health) = allocate_server_ports(&registry, &user_root).unwrap();
        assert_ne!(ingress.port(), 18_080);
        assert_ne!(health.port(), 18_081);
    }
}
