use std::{
    fs,
    path::{Path, PathBuf},
    process::Output,
    time::Duration,
};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::{process::Command, time::timeout};
use uuid::Uuid;

use crate::config::CodexConfig;

const REQUIRED_CLIENT_METHODS: &[&str] = &[
    "initialize",
    "thread/fork",
    "thread/inject_items",
    "thread/resume",
    "thread/start",
    "turn/interrupt",
    "turn/start",
    "turn/steer",
];
const REQUIRED_SERVER_METHODS: &[&str] =
    &["item/completed", "item/started", "thread/status/changed", "turn/completed", "turn/started"];

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("Codex executable does not exist: {0}")]
    MissingExecutable(PathBuf),
    #[error("cannot launch Codex: {0}")]
    Launch(#[from] std::io::Error),
    #[error("Codex command timed out")]
    Timeout,
    #[error("Codex command failed: {0}")]
    Command(String),
    #[error("generated Codex schema is missing {0}")]
    MissingSchema(PathBuf),
    #[error("cannot parse generated Codex schema {path}: {source}")]
    SchemaJson { path: PathBuf, source: serde_json::Error },
    #[error("generated Codex schema lacks required methods: client={client:?}, server={server:?}")]
    MissingMethods { client: Vec<String>, server: Vec<String> },
    #[error("generated experimental TurnStartParams lacks additionalContext")]
    MissingApplicationContext,
    #[error("Codex protocol identity differs from configured pins")]
    IdentityMismatch,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolIdentity {
    pub version: String,
    pub stable_schema_sha256: String,
    pub experimental_schema_sha256: String,
}

pub async fn inspect_codex(config: &CodexConfig) -> Result<ProtocolIdentity, ProtocolError> {
    if !config.executable.is_file() {
        return Err(ProtocolError::MissingExecutable(config.executable.clone()));
    }
    let version = run(&config.executable, &["--version"]).await?;
    let root = std::env::temp_dir().join(format!("braid-schema-{}", Uuid::now_v7()));
    let stable = root.join("stable");
    let experimental = root.join("experimental");
    fs::create_dir_all(&stable)?;
    fs::create_dir_all(&experimental)?;
    let result = async {
        run(
            &config.executable,
            &[
                "app-server",
                "generate-json-schema",
                "--out",
                stable.to_str().ok_or_else(|| {
                    ProtocolError::Command("temporary schema path is not UTF-8".into())
                })?,
            ],
        )
        .await?;
        run(
            &config.executable,
            &[
                "app-server",
                "generate-json-schema",
                "--experimental",
                "--out",
                experimental.to_str().ok_or_else(|| {
                    ProtocolError::Command("temporary schema path is not UTF-8".into())
                })?,
            ],
        )
        .await?;
        verify_methods(&stable)?;
        verify_methods(&experimental)?;
        verify_application_context(&experimental)?;
        Ok(ProtocolIdentity {
            version,
            stable_schema_sha256: digest_bundle(&stable)?,
            experimental_schema_sha256: digest_bundle(&experimental)?,
        })
    }
    .await;
    let _ = fs::remove_dir_all(root);
    result
}

pub fn verify_identity(
    actual: &ProtocolIdentity,
    expected: &CodexConfig,
) -> Result<(), ProtocolError> {
    if actual.version == expected.version
        && actual.stable_schema_sha256 == expected.stable_schema_sha256
        && actual.experimental_schema_sha256 == expected.experimental_schema_sha256
    {
        Ok(())
    } else {
        Err(ProtocolError::IdentityMismatch)
    }
}

async fn run(executable: &Path, arguments: &[&str]) -> Result<String, ProtocolError> {
    let output =
        timeout(Duration::from_secs(30), Command::new(executable).args(arguments).output())
            .await
            .map_err(|_| ProtocolError::Timeout)??;
    output_text(&output)
}

fn output_text(output: &Output) -> Result<String, ProtocolError> {
    if !output.status.success() {
        return Err(ProtocolError::Command(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn verify_methods(directory: &Path) -> Result<(), ProtocolError> {
    let client_path = directory.join("ClientRequest.json");
    let server_path = directory.join("ServerNotification.json");
    let client = read_json(&client_path)?;
    let server = read_json(&server_path)?;
    let client_text = client.to_string();
    let server_text = server.to_string();
    let missing_client = REQUIRED_CLIENT_METHODS
        .iter()
        .filter(|method| !client_text.contains(&format!("\"{method}\"")))
        .map(|method| (*method).to_owned())
        .collect::<Vec<_>>();
    let missing_server = REQUIRED_SERVER_METHODS
        .iter()
        .filter(|method| !server_text.contains(&format!("\"{method}\"")))
        .map(|method| (*method).to_owned())
        .collect::<Vec<_>>();
    if missing_client.is_empty() && missing_server.is_empty() {
        Ok(())
    } else {
        Err(ProtocolError::MissingMethods { client: missing_client, server: missing_server })
    }
}

fn verify_application_context(directory: &Path) -> Result<(), ProtocolError> {
    let path = directory.join("v2/TurnStartParams.json");
    let schema = read_json(&path)?;
    if schema.get("properties").and_then(|properties| properties.get("additionalContext")).is_some()
    {
        Ok(())
    } else {
        Err(ProtocolError::MissingApplicationContext)
    }
}

fn digest_bundle(directory: &Path) -> Result<String, ProtocolError> {
    let path = directory.join("codex_app_server_protocol.v2.schemas.json");
    let bytes = fs::read(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProtocolError::MissingSchema(path.clone())
        } else {
            ProtocolError::Launch(error)
        }
    })?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn read_json(path: &Path) -> Result<Value, ProtocolError> {
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ProtocolError::MissingSchema(path.to_path_buf())
        } else {
            ProtocolError::Launch(error)
        }
    })?;
    serde_json::from_slice(&bytes)
        .map_err(|source| ProtocolError::SchemaJson { path: path.to_path_buf(), source })
}
