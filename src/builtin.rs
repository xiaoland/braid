//! HTTP adapter for the built-in collaboration authority. Its ledger belongs to the Harness.
use crate::{
    config::Config,
    context::CanonicalContext,
    platform::{execution::FenceExecution, grants::GrantEpoch},
    store::{
        BuiltinBinding, BuiltinCommand, BuiltinExecutionCommand, BuiltinGrant, BuiltinReply,
        BuiltinWriterTurn, EventKind, IngressEvent, PendingGitHubWrite, RuntimeLease, StoreActor,
    },
};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::sync::Arc;

#[derive(Clone)]
pub(crate) struct BuiltinClient {
    http: reqwest::Client,
    endpoint: url::Url,
    pub binding: BuiltinBinding,
    pub store: Arc<StoreActor>,
    run_id: Option<String>,
    execution_fences: bool,
    runtime: Option<RuntimeLease>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryInfo {
    pub repository: String,
    pub node_id: String,
    pub space_id: String,
    pub default_branch: String,
    #[serde(default)]
    pub execution_fences: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventFeed {
    pub events: Vec<PlatformEvent>,
    pub next_after: u64,
    pub last_sequence: u64,
}
#[derive(Deserialize)]
pub(crate) struct PlatformEvent {
    pub sequence: u64,
    pub event: WireEvent,
}
#[derive(Deserialize, Serialize)]
pub(crate) struct WireEvent {
    pub delivery_guid: String,
    pub event_name: String,
    pub action: Option<String>,
    pub repository_node_id: String,
    pub repository: String,
    pub work_item_node_id: Option<String>,
    pub work_item_kind: Option<String>,
    pub work_item_number: Option<u64>,
    pub work_item_state: Option<String>,
    pub object_node_id: Option<String>,
    pub object_version: Option<String>,
    pub object_digest: Option<String>,
    pub visible_body: Option<String>,
    pub actor_node_id: Option<String>,
    pub actor_login: Option<String>,
    pub kind: String,
    pub detail: Option<String>,
    pub cross_surface_invalidation: bool,
    pub origin: String,
    pub reference: String,
    pub mention_candidate: bool,
    pub known: bool,
    pub raw_payload: Vec<u8>,
}
impl WireEvent {
    pub(crate) fn into_ingress(self) -> Result<IngressEvent> {
        let kind =
            EventKind::from_str(&self.kind).context("platform event has an unknown queue kind")?;
        let work_item_kind = match self.work_item_kind.as_deref() {
            None => None,
            Some("issue") => Some("issue"),
            Some("pr") => Some("pr"),
            Some(other) => anyhow::bail!("platform returned unsupported work item kind {other}"),
        };
        let detail = match self.detail.as_deref() {
            None => None,
            Some("closed") => Some("closed"),
            Some("reopened") => Some("reopened"),
            Some("merged") => Some("merged"),
            Some("cross_surface") => Some("cross_surface"),
            Some(other) => anyhow::bail!("platform returned unsupported event detail {other}"),
        };
        let origin = match self.origin.as_str() {
            "human" => "human",
            "agent" => "agent",
            "local" => "local",
            other => anyhow::bail!("platform event has unsupported origin {other}"),
        };
        Ok(IngressEvent {
            identity: crate::store::ChangeIdentity::Occurrence,
            delivery_guid: self.delivery_guid,
            event_name: self.event_name,
            action: self.action,
            repository_node_id: self.repository_node_id,
            repository: self.repository,
            work_item_node_id: self.work_item_node_id,
            work_item_kind,
            work_item_number: self.work_item_number,
            work_item_state: if work_item_kind == Some("pr") && detail == Some("merged") {
                Some("merged".into())
            } else {
                self.work_item_state
            },
            object_node_id: self.object_node_id,
            object_version: self.object_version,
            object_digest: self.object_digest,
            visible_body: self.visible_body,
            actor_node_id: self.actor_node_id,
            actor_login: self.actor_login,
            kind,
            detail,
            cross_surface_invalidation: self.cross_surface_invalidation,
            requires_attention: origin != "agent"
                && matches!(kind, EventKind::Wake | EventKind::Invalidate | EventKind::Mention),
            origin,
            reference: self.reference,
            mention_candidate: self.mention_candidate,
            reaction_target: None,
            known: self.known,
            raw_payload: self.raw_payload,
        })
    }
}
#[derive(Deserialize)]
#[serde(tag = "lifecycle", rename_all = "snake_case", rename_all_fields = "camelCase")]
enum MintReceipt {
    Active { grant_id: String, epoch: u64, token: String },
    Denied { reason: String },
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<&'a str>,
    request_id: &'a str,
    client_id: &'a str,
    item_number: u64,
    actor: &'a str,
    expected_epoch: u64,
    authority_cursor: u64,
    execution_id: &'a str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutboxRequest<'a> {
    request_id: &'a str,
    client_id: &'a str,
    repository: &'a str,
    target_kind: &'a str,
    target_database_id: &'a str,
    operation: &'a str,
    content: &'a str,
    remote_database_id: Option<&'a str>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppliedWrite {
    database_id: Option<String>,
    node_id: Option<String>,
}

impl BuiltinClient {
    pub(crate) async fn connect(config: &Config, store: Arc<StoreActor>) -> Result<Arc<Self>> {
        let settings =
            config.builtin.as_ref().context("built-in upstream configuration missing")?;
        let http =
            reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build()?;
        let response = http.get(settings.endpoint.join("api/platform/repository")?).send().await?;
        let info: RepositoryInfo = decode(response).await?;
        ensure!(
            info.repository == settings.repository,
            "built-in upstream repository mismatch: expected {}, observed {}",
            settings.repository,
            info.repository
        );
        ensure!(
            info.node_id == format!("local:{}", info.repository),
            "built-in upstream returned inconsistent repository identity"
        );
        ensure!(!info.space_id.is_empty(), "built-in upstream has no durable space identity");
        let BuiltinReply::Binding(binding) = store
            .builtin(BuiltinCommand::Bind {
                space_id: info.space_id,
                repository: info.repository,
                instance: config.instance_key().into(),
            })
            .map_err(|error| {
                let extended_code = match &error {
                    crate::store::StoreError::Sqlite(rusqlite::Error::SqliteFailure(code, _)) => {
                        Some(code.extended_code)
                    }
                    _ => None,
                };
                anyhow::Error::new(error).context(format!(
                    "bind built-in upstream to Harness ledger {} (SQLite extended code {extended_code:?})",
                    config.runtime.database().display()
                ))
            })?
        else {
            unreachable!()
        };
        Ok(Arc::new(Self {
            http,
            endpoint: settings.endpoint.clone(),
            binding,
            store,
            run_id: config.finite_run.as_ref().map(|run| run.run_id.clone()),
            execution_fences: info.execution_fences,
            runtime: None,
        }))
    }
    pub(crate) fn with_runtime(&self, lease: RuntimeLease) -> Result<Arc<Self>> {
        ensure!(self.execution_fences, "built-in platform must support execution fences");
        Ok(Arc::new(Self { runtime: Some(lease), ..self.clone() }))
    }

    fn runtime(&self) -> Result<RuntimeLease> {
        self.runtime.clone().context("built-in execution requires the owning runtime lease")
    }

    pub(crate) fn bind_execution(&self, turn_id: &str) -> Result<()> {
        self.store.builtin(BuiltinCommand::Execution(BuiltinExecutionCommand::BindTurn {
            lease: self.runtime()?,
            turn_id: turn_id.into(),
        }))?;
        Ok(())
    }

    pub(crate) async fn fence_session(&self, provider_session: &str) -> Result<()> {
        self.fence(BuiltinExecutionCommand::FenceSession {
            lease: self.runtime()?,
            provider_session: provider_session.into(),
        })
        .await
    }

    pub(crate) async fn fence_all_executions(&self) -> Result<()> {
        self.fence(BuiltinExecutionCommand::FenceAll(self.runtime()?)).await
    }

    async fn fence(&self, command: BuiltinExecutionCommand) -> Result<()> {
        let lease = self.runtime()?;
        let BuiltinReply::Fences(executions) =
            self.store.builtin(BuiltinCommand::Execution(command))?
        else {
            unreachable!()
        };
        let mut failures = Vec::new();
        for execution_id in executions {
            if let Err(error) = self.fence_execution(&lease, execution_id).await {
                failures.push(format!("{error:#}"));
            }
        }
        ensure!(failures.is_empty(), "execution fence unconfirmed: {}", failures.join("\n"));
        Ok(())
    }

    async fn fence_execution(&self, lease: &RuntimeLease, execution_id: String) -> Result<()> {
        let request = FenceExecution { client_id: self.binding.client_id.clone(), execution_id };
        let confirmation: FenceExecution = self
            .post("api/platform/executions/fence", &request)
            .await
            .with_context(|| format!("cannot fence execution {}", request.execution_id))?;
        ensure!(confirmation == request, "platform confirmed a different execution fence");
        self.store.builtin(BuiltinCommand::Execution(BuiltinExecutionCommand::ConfirmFence {
            lease: lease.clone(),
            execution_id: request.execution_id,
        }))?;
        Ok(())
    }

    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        decode(self.http.get(self.endpoint.join(path)?).send().await?).await
    }
    pub(crate) async fn scope_boundary(
        &self,
        root: u64,
    ) -> Result<crate::platform::objects::ScopeBoundary> {
        self.get(&format!("api/platform/scope/{root}/boundary")).await
    }
    pub(crate) async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T> {
        decode(self.http.post(self.endpoint.join(path)?).json(body).send().await?).await
    }
    pub(crate) async fn context(&self, number: u64) -> Result<CanonicalContext> {
        self.get(&format!("api/platform/context/{number}")).await
    }
    pub(crate) async fn execution_context(&self, number: u64) -> Result<CanonicalContext> {
        let mut url = self.endpoint.join(&format!("api/platform/context/{number}"))?;
        url.query_pairs_mut().append_pair("executionClientId", &self.binding.client_id);
        if let Some(run_id) = &self.run_id {
            url.query_pairs_mut().append_pair("executionRunId", run_id);
        }
        self.get(url.as_str()).await
    }
    pub(crate) async fn item(&self, number: u64) -> Result<crate::platform::objects::Item> {
        let item = self
            .get::<crate::platform::objects::Item>(&format!("api/platform/items/{number}"))
            .await?;
        ensure!(item.number == number, "platform returned a different item identity");
        Ok(item)
    }
    pub(crate) async fn default_branch(&self) -> Result<String> {
        Ok(self.get::<RepositoryInfo>("api/platform/repository").await?.default_branch)
    }
    pub(crate) async fn candidate(&self, number: u64) -> Result<String> {
        let item = self.item(number).await?;
        ensure!(item.kind == "pr", "platform item #{number} is not a PR");
        item.head_commit.context("platform PR has no frozen candidate commit")
    }
    pub(crate) async fn writer_grant(&self, turn: BuiltinWriterTurn) -> Result<BuiltinGrant> {
        let existing = self.local_writer_grant(&turn)?;
        if let Some(grant) = &existing
            && grant.lifecycle == "pending"
        {
            self.activate(grant).await?;
        } else {
            let mut url = self.endpoint.join("api/platform/grants")?;
            url.query_pairs_mut()
                .append_pair("clientId", &self.binding.client_id)
                .append_pair("itemNumber", &turn.item_number.to_string())
                .append_pair("actor", &turn.actor);
            let remote: Option<GrantEpoch> = decode(self.http.get(url).send().await?).await?;
            if let Some(remote) = remote.as_ref().filter(|remote| remote.active) {
                // A concurrent CLI may have acquired the shared capability while
                // the epoch read was in flight. Only replay its exact local intent.
                match self.local_writer_grant(&turn)? {
                    Some(grant) if grant.lifecycle == "pending" => self.activate(&grant).await?,
                    Some(grant) if grant.lifecycle == "active" && grant.epoch == remote.epoch => {}
                    _ => anyhow::bail!(
                        "platform already has an active writer capability for this responsibility"
                    ),
                }
            } else {
                let expected_epoch = remote.map_or(0, |remote| remote.epoch);
                let BuiltinReply::Grant(Some(grant)) = self
                    .store
                    .builtin(BuiltinCommand::BeginMint { turn: turn.clone(), expected_epoch })?
                else {
                    unreachable!()
                };
                if grant.lifecycle == "pending" {
                    self.activate(&grant).await?;
                }
            }
        }
        let grant = self
            .local_writer_grant(&turn)?
            .context("writer capability disappeared during admission")?;
        ensure!(grant.lifecycle == "active", "writer capability is not active");
        Ok(grant)
    }

    fn local_writer_grant(&self, turn: &BuiltinWriterTurn) -> Result<Option<BuiltinGrant>> {
        let BuiltinReply::Grant(grant) =
            self.store.builtin(BuiltinCommand::WriterGrant(turn.clone()))?
        else {
            unreachable!()
        };
        Ok(grant)
    }

    async fn activate(&self, grant: &BuiltinGrant) -> Result<()> {
        let cursor = grant.authority_cursor.context("writer intent predates authority fencing")?;
        let request = GrantRequest {
            run_id: self.run_id.as_deref(),
            request_id: &grant.request_id,
            client_id: &self.binding.client_id,
            item_number: grant.item_number,
            actor: &grant.actor,
            expected_epoch: grant.epoch,
            authority_cursor: cursor,
            execution_id: &grant.execution_id,
        };
        match self.post("api/platform/grants", &request).await? {
            MintReceipt::Active { grant_id, epoch, token } => {
                self.store.builtin(BuiltinCommand::Activate {
                    request_id: grant.request_id.clone(),
                    session_id: grant.session_id.clone(),
                    grant_id,
                    epoch,
                    token,
                })?;
            }
            MintReceipt::Denied { reason } => {
                self.store.builtin(BuiltinCommand::RejectMint {
                    request_id: grant.request_id.clone(),
                    session_id: grant.session_id.clone(),
                })?;
                anyhow::bail!("{reason}");
            }
        }
        Ok(())
    }

    pub(crate) async fn apply_write(
        &self,
        write: &PendingGitHubWrite,
    ) -> Result<(Option<String>, Option<String>)> {
        let request = OutboxRequest {
            request_id: &write.intent_id,
            client_id: &self.binding.client_id,
            repository: &write.repository,
            target_kind: &write.target_kind,
            target_database_id: &write.target_database_id,
            operation: &write.operation,
            content: &write.content,
            remote_database_id: write.remote_database_id.as_deref(),
        };
        let result: AppliedWrite = self.post("api/platform/writes", &request).await?;
        Ok((result.database_id, result.node_id))
    }
}
#[derive(Debug, thiserror::Error)]
#[error("built-in platform HTTP {status}: {body}")]
pub(crate) struct PlatformHttpError {
    pub status: reqwest::StatusCode,
    pub body: String,
}
async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let status = response.status();
    let body = response.text().await?;
    if !status.is_success() {
        return Err(PlatformHttpError { status, body }.into());
    }
    serde_json::from_str(&body)
        .with_context(|| format!("invalid built-in platform HTTP {status} response: {body}"))
}
