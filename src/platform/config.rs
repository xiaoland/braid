use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformConfig {
    pub schema_version: u32,
    pub repository: String,
    pub source: PathBuf,
    pub default_branch: String,
    pub database: PathBuf,
    pub backups: PathBuf,
    #[serde(default = "listen")]
    pub listen: String,
}
fn listen() -> String {
    "127.0.0.1:18082".into()
}
impl PlatformConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config: Self = toml::from_str(
            &std::fs::read_to_string(path)
                .with_context(|| format!("read platform config {}", path.display()))?,
        )?;
        let parent = path.canonicalize()?.parent().context("platform config parent")?.to_path_buf();
        for value in [&mut config.source, &mut config.database, &mut config.backups] {
            if value.is_relative() {
                *value = parent.join(&*value);
            }
        }
        config.source = config.source.canonicalize()?;
        config.database = resolve_storage(&config.database)?;
        config.backups = resolve_storage(&config.backups)?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported platform configuration version {}",
            self.schema_version
        );
        ensure!(
            !self.repository.trim().is_empty() && !self.default_branch.trim().is_empty(),
            "repository and default_branch must not be empty"
        );
        let address: SocketAddr = self.listen.parse()?;
        ensure!(address.ip().is_loopback(), "platform must listen on loopback");
        ensure!(
            self.source.is_absolute() && self.database.is_absolute() && self.backups.is_absolute(),
            "platform storage declarations must resolve to absolute paths"
        );
        ensure!(
            self.source != self.database
                && self.database != self.backups
                && self.source != self.backups,
            "source, database and backups must have distinct paths"
        );
        ensure!(!self.database.is_dir(), "platform database path is a directory");
        ensure!(!self.backups.is_file(), "platform backups path is a file");
        let git = git2::Repository::open(&self.source)?;
        ensure!(!git.is_bare(), "platform source must have a worktree");
        ensure!(
            git.workdir().context("platform source worktree root")?.canonicalize()? == self.source,
            "platform source must name its Git worktree root"
        );
        let branch = if self.default_branch.starts_with("refs/heads/") {
            self.default_branch.clone()
        } else {
            format!("refs/heads/{}", self.default_branch)
        };
        ensure!(git2::Reference::is_valid_name(&branch), "invalid default branch");
        git.find_reference(&branch)?.peel_to_commit()?;
        Ok(())
    }
}

fn resolve_storage(path: &Path) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                ensure!(normalized.pop(), "storage path escapes its root");
            }
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    if normalized.exists() {
        return Ok(normalized.canonicalize()?);
    }
    let parent = normalized.parent().context("storage path has no parent")?;
    let name = normalized.file_name().context("storage path has no file name")?;
    Ok(resolve_storage(parent)?.join(name))
}
