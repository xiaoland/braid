use super::PlatformConfig;
use anyhow::{Result, ensure};
use clap::{Args, Subcommand};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt as _;
use std::{io::Write as _, path::PathBuf};
#[derive(Debug, Args)]
pub struct PlatformArguments {
    #[command(subcommand)]
    pub command: PlatformCommand,
}
#[derive(Debug, Subcommand)]
pub enum PlatformCommand {
    Init {
        #[arg(long)]
        config: PathBuf,
        #[arg(long = "name", alias = "repository")]
        repository: String,
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        default_branch: Option<String>,
        #[arg(long, default_value = "127.0.0.1:18082")]
        listen: String,
    },
    Serve {
        #[arg(long)]
        config: PathBuf,
    },
    Import {
        #[arg(long)]
        config: PathBuf,
        #[arg(long = "from-runtime-db", alias = "from")]
        from: PathBuf,
    },
}
pub async fn run(arguments: PlatformArguments) -> Result<()> {
    match arguments.command {
        PlatformCommand::Init { config, repository, source, default_branch, listen } => {
            ensure!(!config.exists(), "platform configuration already exists");
            let parent = config.parent().unwrap_or(std::path::Path::new("."));
            std::fs::create_dir_all(parent)?;
            let parent = parent.canonicalize()?;
            let source = source.canonicalize()?;
            let default_branch = default_branch.map_or_else(
                || {
                    let git = git2::Repository::open(&source)?;
                    let head = git.head()?;
                    ensure!(
                        head.is_branch(),
                        "source checkout is detached; pass --default-branch explicitly"
                    );
                    head.shorthand().map(str::to_owned).map_err(anyhow::Error::from)
                },
                Ok,
            )?;
            let declaration = PlatformConfig {
                schema_version: 1,
                repository,
                source,
                default_branch,
                database: parent.join("platform.sqlite3"),
                backups: parent.join("platform-backups"),
                listen,
            };
            declaration.validate()?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options.open(&config)?;
            file.write_all(toml::to_string_pretty(&declaration)?.as_bytes())?;
            file.sync_all()?;
            println!("platform configuration: {}", config.display());
        }
        PlatformCommand::Serve { config } => {
            let config = PlatformConfig::load(&config)?;
            let (stop, receiver) = tokio::sync::watch::channel(false);
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                let _ = stop.send(true);
            });
            super::serve(config, receiver).await?;
        }
        PlatformCommand::Import { config, from } => {
            super::store::import(&PlatformConfig::load(&config)?, &from)?;
            println!("platform objects imported; source database preserved");
        }
    }
    Ok(())
}
