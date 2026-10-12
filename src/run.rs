//! A bounded root-task execution mode over the existing built-in Harness.
use crate::{
    config::Config,
    health::HealthSnapshot,
    platform::objects::{CommandRequest, ControlReceipt, ObjectCommand},
    store::{RunCommand, StoreActor},
};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{RwLock, Semaphore, watch};

#[derive(Debug, Clone)]
pub(crate) struct RunContext {
    pub run_id: String,
    pub root: u64,
    pub delivery_ref: String,
    pub admission: Arc<Semaphore>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RunRequest {
    pub run_id: String,
    pub root_issue: Option<u64>,
    pub description: Option<String>,
    pub delivery_ref: String,
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
}
#[derive(Debug, thiserror::Error)]
#[error("same run cannot resume with a different Harness configuration")]
struct ConfigurationChanged;
#[derive(Debug, thiserror::Error)]
#[error("previous run stop is unconfirmed; repair execution ownership before resuming")]
struct StopUnconfirmed;
struct PreparationLease {
    store: Arc<StoreActor>,
    lease: crate::store::RuntimeLease,
}
impl Drop for PreparationLease {
    fn drop(&mut self) {
        if let Err(error) = self.store.release_runtime_lease(self.lease.clone()) {
            tracing::error!(%error,"cannot release finite run preparation lease");
        }
    }
}
fn default_concurrency() -> usize {
    2
}
impl RunRequest {
    pub(crate) fn validate(&self, config: &Config) -> Result<()> {
        ensure!(config.is_builtin(), "braid run currently requires the built-in upstream");
        ensure!(
            !self.run_id.is_empty()
                && self.run_id.len() <= 96
                && self.run_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "runId must be 1–96 letters, digits, hyphens or underscores"
        );
        ensure!(
            self.root_issue.is_some() != self.description.is_some(),
            "provide exactly one of rootIssue or description"
        );
        ensure!(self.root_issue != Some(0), "rootIssue must be positive");
        ensure!(
            self.description.as_ref().is_none_or(|s| !s.trim().is_empty()),
            "description must not be empty"
        );
        ensure!(
            (1..=64).contains(&self.max_concurrency),
            "maxConcurrency must be between 1 and 64"
        );
        ensure!(
            self.delivery_ref.starts_with("refs/heads/") && self.delivery_ref.len() > 11,
            "deliveryRef must be an explicit refs/heads/... reference"
        );
        let status = std::process::Command::new(&config.tools.git)
            .args(["check-ref-format", &self.delivery_ref])
            .status()
            .context("validate delivery Git ref")?;
        ensure!(status.success(), "deliveryRef is not a valid Git reference");
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RunState {
    Running,
    Completed,
    Blocked,
    Failed,
    Stopped,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunResult {
    pub run_id: String,
    pub root_issue: u64,
    pub state: RunState,
    pub delivery_commit: Option<String>,
    pub reason: Option<String>,
    pub stop_confirmed: bool,
}
fn directory(config: &Config, id: &str) -> Result<PathBuf> {
    ensure!(
        !id.is_empty()
            && id.len() <= 96
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid run identity"
    );
    Ok(config.runtime.root.as_ref().context("runtime root is unresolved")?.join("runs").join(id))
}
pub(crate) fn run_snapshot(config: &Config, id: &str) -> Result<Option<RunResult>> {
    let root = directory(config, id)?;
    if !root.join("braid.sqlite3").is_file() {
        return Ok(None);
    }
    use rusqlite::OptionalExtension as _;
    let connection = rusqlite::Connection::open_with_flags(
        root.join("braid.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let raw: Option<String> = connection
        .query_row("SELECT result_json FROM harness_runs WHERE run_id=?1", [id], |row| row.get(0))
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).context("decode run result")).transpose()
}
pub(crate) fn save(store: &StoreActor, request: &RunRequest, result: &RunResult) -> Result<()> {
    store.run_record(RunCommand::Save {
        run_id: request.run_id.clone(),
        request: serde_json::to_string(request)?,
        result: serde_json::to_string(result)?,
    })?;
    Ok(())
}

pub(crate) fn run_database(config: &Config, id: &str) -> Result<PathBuf> {
    Ok(directory(config, id)?.join("braid.sqlite3"))
}

pub(crate) async fn run_managed(
    config: Config,
    request: RunRequest,
    stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<RunResult> {
    request.validate(&config)?;
    match run_inner(config.clone(), request.clone(), stop, health).await {
        Ok(result) => Ok(result),
        Err(error) => {
            if error.downcast_ref::<ConfigurationChanged>().is_some()
                || error.downcast_ref::<StopUnconfirmed>().is_some()
                || matches!(
                    error.downcast_ref::<crate::store::StoreError>(),
                    Some(crate::store::StoreError::RuntimeBusy { .. })
                )
            {
                return Err(error);
            }
            let previous = run_snapshot(&config, &request.run_id)?;
            let result = RunResult {
                run_id: request.run_id.clone(),
                root_issue: previous
                    .as_ref()
                    .map_or(request.root_issue.unwrap_or(0), |r| r.root_issue),
                state: RunState::Failed,
                delivery_commit: None,
                reason: Some(format!("{error:#}")),
                stop_confirmed: previous.as_ref().is_none_or(|r| r.stop_confirmed),
            };
            let root = directory(&config, &request.run_id)?;
            if root.join("braid.sqlite3").exists() {
                let store = StoreActor::start(root.join("braid.sqlite3"), root.join("backups"))?;
                save(&store, &request, &result)?;
            }
            Ok(result)
        }
    }
}

async fn run_inner(
    mut config: Config,
    request: RunRequest,
    stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<RunResult> {
    request.validate(&config)?;
    let root = directory(&config, &request.run_id)?;
    std::fs::create_dir_all(&root)?;
    config.runtime.root = Some(root.clone());
    config.runtime.database = Some(root.join("braid.sqlite3"));
    config.runtime.backups = Some(root.join("backups"));
    config.runtime.worktrees = Some(root.join("worktrees"));
    config.loaded_from = root.join("config.toml");
    let frozen = toml::to_string_pretty(&config)?.into_bytes();
    if config.loaded_from.exists() {
        if std::fs::read(&config.loaded_from)? != frozen {
            return Err(ConfigurationChanged.into());
        }
    } else {
        use std::io::Write as _;
        let temporary = root.join(format!("config-{}.toml.pending", uuid::Uuid::now_v7()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&frozen)?;
        file.sync_all()?;
        let published = std::fs::hard_link(&temporary, &config.loaded_from);
        std::fs::remove_file(&temporary)?;
        if let Err(error) = published {
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error.into());
            }
            if std::fs::read(&config.loaded_from)? != frozen {
                return Err(ConfigurationChanged.into());
            }
        }
    }
    let store = Arc::new(StoreActor::start(
        config.runtime.database().into(),
        config.runtime.backups().into(),
    )?);
    let migration = store.plan()?;
    if !migration.pending.is_empty() {
        ensure!(
            migration.current_schema == 0 || config.runtime.auto_migrate,
            "existing run ledger requires migration; use its frozen config with braid migrate apply"
        );
        store.apply()?;
    }
    let preparation_lease = store.acquire_runtime_lease(
        "run-preparation".into(),
        uuid::Uuid::now_v7().to_string(),
        300,
    )?;
    let _preparation_guard =
        PreparationLease { store: Arc::clone(&store), lease: preparation_lease };
    let previous = store
        .run_record(RunCommand::Read(request.run_id.clone()))?
        .map(|raw| serde_json::from_str::<RunResult>(&raw))
        .transpose()?;
    if let Some(result) = &previous {
        // Verify input identity even when a completed run is read again.
        save(&store, &request, result)?;
        if result.state == RunState::Completed {
            return Ok(result.clone());
        }
        if !result.stop_confirmed {
            return Err(StopUnconfirmed.into());
        }
    }
    if *stop.borrow() {
        let result = RunResult {
            run_id: request.run_id.clone(),
            root_issue: previous.as_ref().map_or(request.root_issue.unwrap_or(0), |r| r.root_issue),
            state: RunState::Stopped,
            delivery_commit: None,
            reason: Some("stopped before execution".into()),
            stop_confirmed: true,
        };
        save(&store, &request, &result)?;
        return Ok(result);
    }
    let client = crate::builtin::BuiltinClient::connect(&config, Arc::clone(&store)).await?;
    let root_issue = if let Some(result) = previous.filter(|result| result.root_issue > 0) {
        result.root_issue
    } else if let Some(number) = request.root_issue {
        number
    } else {
        let description = request.description.as_ref().expect("validated description");
        let receipt: ControlReceipt = client
            .post(
                "api/control/commands",
                &CommandRequest {
                    request_id: format!("run-{}-root", request.run_id),
                    expected_revision: None,
                    command: ObjectCommand::CreateIssue {
                        title: description
                            .lines()
                            .find(|line| !line.trim().is_empty())
                            .unwrap_or("Root task")
                            .trim()
                            .chars()
                            .take(120)
                            .collect(),
                        body: description.clone(),
                        parent: None,
                        parent_revision: None,
                    },
                    actor: "human".into(),
                    writer: None,
                },
            )
            .await?;
        ensure!(receipt.state == "applied", "root creation {}: {:?}", receipt.state, receipt.error);
        receipt
            .result
            .as_ref()
            .and_then(|value| value.get("number"))
            .and_then(serde_json::Value::as_u64)
            .context("root creation receipt lacks object number")?
    };
    let mut result = RunResult {
        run_id: request.run_id.clone(),
        root_issue,
        state: RunState::Running,
        delivery_commit: None,
        reason: Some("preparing root execution".into()),
        stop_confirmed: true,
    };
    save(&store, &request, &result)?;
    let scope: crate::platform::objects::ScopeSnapshot =
        client.get(&format!("api/platform/scope/{root_issue}")).await?;
    if scope.root.state != "open" {
        let result = RunResult {
            run_id: request.run_id.clone(),
            root_issue,
            state: RunState::Blocked,
            delivery_commit: None,
            reason: Some("root is closed; explicitly reopen it before starting or resuming".into()),
            stop_confirmed: true,
        };
        save(&store, &request, &result)?;
        return Ok(result);
    }
    if scope.delivery_ref.as_deref() != Some(&request.delivery_ref)
        || scope.run_id.as_deref() != Some(&request.run_id)
        || scope.client_id.as_deref() != Some(&client.binding.client_id)
    {
        let configured: ControlReceipt = client
            .post(
                "api/control/commands",
                &CommandRequest {
                    request_id: format!("run-{}-boundary", request.run_id),
                    expected_revision: Some(scope.root.revision),
                    command: ObjectCommand::ConfigureRunRoot {
                        number: root_issue,
                        client_id: client.binding.client_id.clone(),
                        run_id: request.run_id.clone(),
                        delivery_ref: request.delivery_ref.clone(),
                    },
                    actor: "human".into(),
                    writer: None,
                },
            )
            .await?;
        ensure!(
            configured.state == "applied",
            "root boundary configuration {}: {:?}",
            configured.state,
            configured.error
        );
    }
    let scope: crate::platform::objects::ScopeSnapshot =
        client.get(&format!("api/platform/scope/{root_issue}")).await?;
    let issue_profile = config
        .profiles
        .iter()
        .find(|p| p.has_tag("issue"))
        .context("configuration lacks Issue profile")?;
    if scope.root.desired_profile.as_deref() != Some(&issue_profile.id) {
        let receipt: ControlReceipt = client
            .post(
                "api/control/commands",
                &CommandRequest {
                    request_id: format!("run-{}-assign-{}", request.run_id, scope.root.revision),
                    expected_revision: Some(scope.root.revision),
                    command: ObjectCommand::Assign {
                        number: root_issue,
                        actor: issue_profile.id.clone(),
                    },
                    actor: "human".into(),
                    writer: None,
                },
            )
            .await?;
        ensure!(
            receipt.state == "applied",
            "root assignment {}: {:?}",
            receipt.state,
            receipt.error
        );
    }
    result.reason = None;
    result.stop_confirmed = false;
    save(&store, &request, &result)?;
    config.finite_run = Some(RunContext {
        run_id: request.run_id.clone(),
        root: root_issue,
        delivery_ref: request.delivery_ref.clone(),
        admission: Arc::new(Semaphore::new(request.max_concurrency)),
    });
    match crate::runtime::builtin_run_managed(
        config,
        request.clone(),
        Arc::clone(&store),
        stop,
        health,
    )
    .await
    {
        Ok(outcome) => result = outcome,
        Err(error) => {
            result.state = RunState::Failed;
            result.reason = Some(format!("{error:#}"));
            result.stop_confirmed = false;
        }
    }
    save(&store, &request, &result)?;
    Ok(result)
}
