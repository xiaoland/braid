use std::path::PathBuf;

use anyhow::{Context as _, Result, bail};

/// Resolve the Braid home directory (`~/.braid`).
pub fn braid_home() -> Result<PathBuf> {
    dirs::home_dir().map(|home| home.join(".braid")).context("cannot determine home directory")
}

/// A single Braid worker: isolated config, secrets, database, worktrees, and logs.
#[derive(Debug, Clone)]
pub struct Worker {
    pub dir: PathBuf,
}

impl Worker {
    /// Resolve a worker by name under `~/.braid/workers/<name>`.
    pub fn from_name(name: &str) -> Result<Self> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            bail!("worker name must be non-empty");
        }
        if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
            bail!("worker name must not contain path separators");
        }
        let dir = braid_home()?.join("workers").join(trimmed);
        Ok(Self { dir })
    }

    pub fn config_path(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn backups_path(&self) -> PathBuf {
        self.dir.join("backups")
    }

    pub fn worktrees_dir(&self) -> PathBuf {
        self.dir.join("worktrees")
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join("logs")
    }

    /// Create the worker directory layout if it does not already exist.
    pub fn ensure_dirs(&self) -> Result<()> {
        for path in [&self.dir, &self.backups_path(), &self.worktrees_dir(), &self.logs_dir()] {
            std::fs::create_dir_all(path)
                .with_context(|| format!("cannot create worker directory {}", path.display()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_paths_resolve_under_braid_home() {
        let worker = Worker::from_name("test-worker").expect("valid worker name");
        assert!(worker.dir.ends_with(".braid/workers/test-worker"));
        assert_eq!(worker.config_path(), worker.dir.join("config.toml"));
    }
}
