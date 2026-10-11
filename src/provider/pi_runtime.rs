use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read as _,
    path::{Component, Path, PathBuf},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{Command, PiConfig, ProviderError, Value};

const CAPABILITIES: &[&str] =
    &["retry_intent", "settle_extensions", "owned_inference", "managed_execution", "owned_stop"];

#[derive(Deserialize)]
struct Artifact {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
struct Plugin {
    name: String,
    extension_paths: Vec<PathBuf>,
    #[serde(default)]
    executable_paths: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct Manifest {
    schema_version: u32,
    capabilities: Vec<String>,
    entrypoint: Artifact,
    managed_module: Artifact,
    process_owner: Artifact,
    node: Artifact,
    files: BTreeMap<PathBuf, String>,
    plugins: Vec<Plugin>,
}

pub(super) struct Material {
    extensions: BTreeSet<PathBuf>,
    node: PathBuf,
    module: PathBuf,
    owner: PathBuf,
}

pub(super) struct Execution {
    pub id: String,
    pub directory: PathBuf,
    extensions: BTreeSet<PathBuf>,
    node: PathBuf,
    module: PathBuf,
    owner: PathBuf,
    start_id: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
pub(super) enum ManagedStatus {
    Quiescent,
    Busy,
    Unknown,
}

#[derive(Deserialize, Debug)]
pub(super) struct ManagedState {
    pub status: ManagedStatus,
    pub execution_id: String,
    pub reason: Option<String>,
}

fn protocol(message: impl Into<String>) -> ProviderError {
    ProviderError::Protocol(message.into())
}

fn package_path(root: &Path, path: &Path) -> Result<PathBuf, ProviderError> {
    if path.as_os_str().is_empty()
        || path.components().any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(protocol(format!(
            "Pi material path must be relative without traversal: {}",
            path.display()
        )));
    }
    let resolved = root.join(path).canonicalize().map_err(|error| {
        protocol(format!("Pi material path {}: {error}", root.join(path).display()))
    })?;
    if !resolved.starts_with(root) {
        return Err(protocol(format!("Pi material path escapes installation: {}", path.display())));
    }
    Ok(resolved)
}

fn verify_file(root: &Path, path: &Path, expected: &str) -> Result<PathBuf, ProviderError> {
    let resolved = package_path(root, path)?;
    let mut file = std::fs::File::open(&resolved)
        .map_err(|error| protocol(format!("Pi material file {}: {error}", resolved.display())))?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 8192];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            protocol(format!("Pi material read {}: {error}", resolved.display()))
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    let actual = hex::encode(digest.finalize());
    if actual != expected {
        return Err(protocol(format!(
            "Pi material SHA-256 mismatch for {}: expected {expected}, found {actual}",
            resolved.display()
        )));
    }
    Ok(resolved)
}

pub(super) fn load(config: &PiConfig) -> Result<Option<Material>, ProviderError> {
    let Some(path) = &config.native_runtime else { return Ok(None) };
    let bytes = std::fs::read(path)
        .map_err(|error| protocol(format!("Pi runtime manifest {}: {error}", path.display())))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|error| protocol(format!("Pi runtime manifest {}: {error}", path.display())))?;
    if manifest.schema_version != 1
        || CAPABILITIES
            .iter()
            .any(|capability| !manifest.capabilities.iter().any(|value| value == capability))
    {
        return Err(protocol(format!(
            "Pi runtime manifest {} lacks the supported lifecycle contract",
            path.display()
        )));
    }
    let root = path
        .parent()
        .ok_or_else(|| protocol("Pi runtime manifest has no installation directory"))?
        .canonicalize()?;
    for (file, hash) in &manifest.files {
        verify_file(&root, file, hash)?;
    }
    let entrypoint = verify_artifact(&root, &manifest, &manifest.entrypoint)?;
    if config.executable.canonicalize()? != entrypoint {
        return Err(protocol(format!(
            "Pi executable {} differs from material entrypoint {}",
            config.executable.display(),
            entrypoint.display()
        )));
    }
    let extensions = plugin_extensions(&root, &manifest)?;
    Ok(Some(Material {
        extensions,
        node: verify_artifact(&root, &manifest, &manifest.node)?,
        module: verify_artifact(&root, &manifest, &manifest.managed_module)?,
        owner: verify_artifact(&root, &manifest, &manifest.process_owner)?,
    }))
}

fn plugin_extensions(root: &Path, manifest: &Manifest) -> Result<BTreeSet<PathBuf>, ProviderError> {
    let mut extensions = BTreeSet::new();
    for plugin in &manifest.plugins {
        if plugin.extension_paths.is_empty() && plugin.executable_paths.is_empty() {
            return Err(protocol(format!(
                "Pi plugin {} declares no extension or executable entrypoint",
                plugin.name
            )));
        }
        for entry in plugin.extension_paths.iter().chain(&plugin.executable_paths) {
            if !manifest.files.contains_key(entry) {
                return Err(protocol(format!(
                    "Pi plugin {} entry {} is not included in material file identities",
                    plugin.name,
                    entry.display()
                )));
            }
            package_path(root, entry)?;
        }
        for extension in &plugin.extension_paths {
            extensions.insert(package_path(root, extension)?);
        }
    }
    Ok(extensions)
}

fn verify_artifact(
    root: &Path,
    manifest: &Manifest,
    artifact: &Artifact,
) -> Result<PathBuf, ProviderError> {
    if manifest.files.get(&artifact.path) != Some(&artifact.sha256) {
        return Err(protocol(format!(
            "Pi material artifact {} is absent from file identities",
            artifact.path.display()
        )));
    }
    package_path(root, &artifact.path)
}

impl Material {
    pub fn execution(&self, workspace: &Path) -> Result<Execution, ProviderError> {
        let id = uuid::Uuid::now_v7().to_string();
        let directory = workspace.join(".braid/native-executions").join(&id);
        std::fs::create_dir_all(&directory)?;
        Ok(Execution {
            id,
            directory: directory.canonicalize()?,
            extensions: self.extensions.clone(),
            node: self.node.clone(),
            module: self.module.clone(),
            owner: self.owner.clone(),
            start_id: uuid::Uuid::now_v7().to_string(),
        })
    }
}

impl Execution {
    pub fn command(&self, executable: &Path) -> Command {
        let mut cmd = Command::new(&self.node);
        cmd.arg(&self.owner)
            .args(["launch", "--start-id", &self.start_id, "--kind", "root", "--"])
            .arg(executable);
        #[cfg(unix)]
        cmd.process_group(0);
        cmd.env_remove("BRAID_NATIVE_START_ID")
            .env("BRAID_NATIVE_LAUNCH_START_ID", &self.start_id)
            .env("BRAID_NATIVE_RUNTIME_MODULE", &self.module)
            .env("BRAID_NATIVE_EXECUTION_DIR", &self.directory)
            .env("BRAID_NATIVE_EXECUTION_ID", &self.id);
        cmd
    }

    pub fn state(&self, response: &Value) -> Result<ManagedState, ProviderError> {
        let runtime = &response["data"]["braid_native_runtime"];
        let capabilities = runtime["capabilities"].as_array().ok_or_else(|| {
            protocol(format!("Pi omitted native runtime capabilities: {response}"))
        })?;
        if runtime["schema_version"] != 1
            || runtime["pi_version"] != "1.0.2"
            || runtime["execution_id"] != self.id
            || CAPABILITIES
                .iter()
                .any(|required| !capabilities.iter().any(|value| value == required))
        {
            return Err(protocol(format!(
                "Pi native runtime contract or execution identity mismatch for {}: {response}",
                self.id
            )));
        }
        let errors = runtime["extension_errors"]
            .as_array()
            .ok_or_else(|| protocol(format!("Pi omitted extension loader outcomes: {response}")))?;
        if !errors.is_empty() {
            return Err(protocol(format!(
                "Pi extension loading failed: {}",
                runtime["extension_errors"]
            )));
        }
        let loaded = runtime["extensions"]
            .as_array()
            .ok_or_else(|| protocol(format!("Pi omitted actual loaded extensions: {response}")))?;
        let loaded: BTreeSet<PathBuf> = loaded
            .iter()
            .map(|value| {
                let path = value.as_str().ok_or_else(|| {
                    protocol(format!("Pi loaded extension identity is not a path: {value}"))
                })?;
                Path::new(path)
                    .canonicalize()
                    .map_err(|error| protocol(format!("Pi loaded extension {path}: {error}")))
            })
            .collect::<Result<_, _>>()?;
        if loaded != self.extensions {
            return Err(protocol(format!(
                "Pi actual loaded extensions differ from material selection: expected {:?}, found {loaded:?}",
                self.extensions
            )));
        }
        let state: ManagedState = serde_json::from_value(response["data"]["managed_state"].clone())
            .map_err(|error| {
                protocol(format!("Pi managed state is invalid: {error}; response={response}"))
            })?;
        if state.execution_id != self.id {
            return Err(protocol(format!(
                "Pi managed state execution mismatch for {}: {response}",
                self.id
            )));
        }
        Ok(state)
    }

    pub async fn cleanup(&self) -> Result<(), ProviderError> {
        let output = super::timeout(
            super::Duration::from_secs(30),
            Command::new(&self.node)
                .arg(&self.module)
                .args(["cleanup", "--execution-dir"])
                .arg(&self.directory)
                .args(["--execution-id", &self.id])
                .output(),
        )
        .await
        .map_err(|_| ProviderError::Timeout {
            method: format!("Pi owned cleanup execution {}", self.id),
        })??;
        let receipt: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
            protocol(format!(
                "Pi owned cleanup receipt invalid: {error}; exit={}; stdout={}; stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ))
        })?;
        tracing::info!(name="braid.provider.execution", provider.kind="pi", provider.execution.id=%self.id, receipt=%receipt, process.exit_status=%output.status, stderr=%String::from_utf8_lossy(&output.stderr), "Pi offline owned execution cleanup");
        if !output.status.success()
            || receipt["status"] != "stopped"
            || receipt["execution_id"] != self.id
            || receipt["scope"] != "execution"
            || receipt["coverage"] != "registered-process-groups-and-inherited-execution-markers"
        {
            return Err(ProviderError::Unknown(format!(
                "Pi owned execution stop unproved: {receipt}; exit={}; stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
}
