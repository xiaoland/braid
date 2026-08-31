use std::{
    io::BufRead as _,
    path::{Path, PathBuf},
    process::Output,
};

use anyhow::{Context as _, Result, bail};
use tokio::process::Command;

use crate::{
    config::{CodexConfig, RuntimeEntry},
    protocol::{self},
};

#[derive(Debug, Clone)]
pub struct DiscoveredRuntime {
    pub adapter_type: String,
    pub version: String,
    pub executable: PathBuf,
    pub source: String,
}

/// Discover locally installed Agent Runtime candidates for the requested
/// adapter type. No installation is performed; if nothing is found the caller
/// prints an install command and exits.
pub async fn discover(adapter_type: &str) -> Result<Vec<DiscoveredRuntime>> {
    let candidates = match adapter_type {
        "pi" => pi_candidates(),
        "codex" => codex_candidates(),
        other => bail!("unsupported adapter_type {other:?}"),
    };

    let mut found = Vec::new();
    for (executable, source) in candidates {
        if !executable.is_file() {
            continue;
        }
        match version_line(&executable).await {
            Ok(version) if !version.is_empty() => {
                found.push(DiscoveredRuntime {
                    adapter_type: adapter_type.to_owned(),
                    version,
                    executable,
                    source,
                });
            }
            Ok(_) => {}
            Err(error) => {
                tracing::debug!(%error, path = %executable.display(), "ignoring candidate");
            }
        }
    }
    Ok(found)
}

/// Verify a discovered or manually supplied runtime and return a populated
/// `RuntimeEntry`. For Codex this runs the schema handshake; for Pi it just
/// re-runs --version.
pub async fn verify(runtime: DiscoveredRuntime) -> Result<RuntimeEntry> {
    if runtime.adapter_type == "codex" {
        let temp_config = CodexConfig {
            executable: runtime.executable.clone(),
            home: runtime.executable.parent().map_or_else(PathBuf::new, Path::to_path_buf),
            version: runtime.version.clone(),
            stable_schema_sha256: String::new(),
            experimental_schema_sha256: String::new(),
        };
        let identity =
            protocol::inspect_codex(&temp_config).await.context("cannot verify Codex runtime")?;
        return Ok(RuntimeEntry {
            adapter_type: runtime.adapter_type,
            version: identity.version,
            executable: runtime.executable,
            api_url: None,
            home: Some(temp_config.home),
            stable_schema_sha256: Some(identity.stable_schema_sha256),
            experimental_schema_sha256: Some(identity.experimental_schema_sha256),
        });
    }

    if runtime.adapter_type == "pi" {
        return Ok(RuntimeEntry {
            adapter_type: runtime.adapter_type,
            version: runtime.version,
            executable: runtime.executable,
            api_url: None,
            home: None,
            stable_schema_sha256: None,
            experimental_schema_sha256: None,
        });
    }

    bail!("unsupported adapter_type {:?}", runtime.adapter_type)
}

/// Build a `RuntimeEntry` from a manually supplied executable path. The path
/// is verified with `--version` and, for Codex, the schema handshake.
pub async fn from_executable(adapter_type: &str, executable: &Path) -> Result<RuntimeEntry> {
    if !executable.is_file() {
        bail!("executable does not exist: {}", executable.display());
    }
    let version = version_line(executable)
        .await
        .with_context(|| format!("cannot run {} --version", executable.display()))?;
    verify(DiscoveredRuntime {
        adapter_type: adapter_type.to_owned(),
        version,
        executable: executable.to_path_buf(),
        source: "--runtime-executable".to_owned(),
    })
    .await
}

/// Build a `RuntimeEntry` from a manual HTTP API URL. No local verification is
/// performed; the adapter is expected to answer at the URL.
pub fn from_api_url(adapter_type: &str, api_url: String) -> Result<RuntimeEntry> {
    if adapter_type != "deepseek-harness" {
        bail!("--runtime-api-url is only supported for deepseek-harness in this release");
    }
    Ok(RuntimeEntry {
        adapter_type: adapter_type.to_owned(),
        version: "manual".to_owned(),
        executable: PathBuf::from("n/a"),
        api_url: Some(api_url),
        home: None,
        stable_schema_sha256: None,
        experimental_schema_sha256: None,
    })
}

pub fn install_hint(adapter_type: &str) -> String {
    match adapter_type {
        "pi" => "Install the Pi CLI:\n\
            pnpm add -g @anthropic-ai/pi-cli   # preferred\n\
            # or: npm install -g @anthropic-ai/pi-cli\n\
            # Then re-run `braid setup`."
            .to_owned(),
        "codex" => "Install the Codex CLI:\n\
            npm install -g codex-cli\n\
            # Then re-run `braid setup`."
            .to_owned(),
        "deepseek-harness" => {
            "Start the DeepSeek harness HTTP runtime and pass --runtime-api-url.".to_owned()
        }
        other => format!("No install instructions for adapter_type {other:?}"),
    }
}

fn pi_candidates() -> Vec<(PathBuf, String)> {
    let mut candidates = Vec::new();
    if let Ok(path) = which::which("pi") {
        candidates.push((path, "PATH".to_owned()));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push((home.join("Library/pnpm/bin/pi"), "pnpm global".to_owned()));
        candidates.push((home.join(".local/share/pnpm/pi"), "pnpm global".to_owned()));
    }
    candidates
}

fn codex_candidates() -> Vec<(PathBuf, String)> {
    let mut candidates = Vec::new();
    if let Ok(path) = which::which("codex") {
        candidates.push((path, "PATH".to_owned()));
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push((
            home.join(".braid/codex-pkg/node_modules/.bin/codex"),
            "local install".to_owned(),
        ));
    }
    candidates
}

async fn version_line(executable: &Path) -> Result<String> {
    let output = run(executable, &["--version"]).await?;
    Ok(output.stdout.lines().map_while(Result::ok).next().unwrap_or_default().trim().to_owned())
}

async fn run(executable: &Path, args: &[&str]) -> Result<Output> {
    Command::new(executable)
        .args(args)
        .output()
        .await
        .with_context(|| format!("cannot spawn {}", executable.display()))
}
