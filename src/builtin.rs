//! HTTP adapter for the built-in collaboration authority. Its ledger belongs to the Harness.
use crate::{
    config::Config,
    context::CanonicalContext,
    store::{
        BuiltinBinding, BuiltinCommand, BuiltinGrant, BuiltinReply, EventKind, IngressEvent,
        PendingGitHubWrite, StoreActor,
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
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RepositoryInfo {
    pub repository: String,
    pub node_id: String,
    pub space_id: String,
    pub default_branch: String,
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
pub(crate) struct ItemEnvelope {
    pub item: PlatformItem,
}
#[derive(Deserialize)]
pub(crate) struct PlatformItem {
    pub number: u64,
    pub kind: String,
    pub revision: u64,
    pub head_commit: Option<String>,
    pub desired_profile: Option<String>,
}
#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WriterCapability {
    pub grant_id: String,
    pub epoch: u64,
    pub token: String,
}
#[derive(Deserialize)]
struct Revoked {
    epoch: u64,
    lifecycle: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    run_id: Option<&'a str>,
    #[serde(skip_serializing_if = "crate::platform::grants::is_false")]
    pending: bool,
    request_id: &'a str,
    client_id: &'a str,
    item_number: u64,
    actor: &'a str,
    expected_epoch: u64,
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
        let BuiltinReply::Binding(binding) = store.builtin(BuiltinCommand::Bind {
            space_id: info.space_id,
            repository: info.repository,
            instance: config.instance_key().into(),
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
        }))
    }
    pub(crate) async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        decode(self.http.get(self.endpoint.join(path)?).send().await?).await
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
    pub(crate) async fn item(&self, number: u64) -> Result<PlatformItem> {
        let item = self.get::<ItemEnvelope>(&format!("api/control/items/{number}")).await?.item;
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
    pub(crate) async fn ensure_grant(
        &self,
        session_id: &str,
        item_number: u64,
        actor: &str,
    ) -> Result<()> {
        let item = self.item(item_number).await?;
        ensure!(
            item.desired_profile.as_deref() == Some(actor),
            "platform assignment no longer belongs to actor {actor}"
        );
        let BuiltinReply::Grant(Some(grant)) = self.store.builtin(BuiltinCommand::Prepare {
            session_id: session_id.into(),
            item_number,
            actor: actor.into(),
        })?
        else {
            anyhow::bail!("grant intent was not persisted")
        };
        if grant.lifecycle == "active" {
            return Ok(());
        }
        self.activate(&grant).await
    }
    async fn activate(&self, grant: &BuiltinGrant) -> Result<()> {
        let request = GrantRequest {
            run_id: self.run_id.as_deref(),
            pending: false,
            request_id: &grant.request_id,
            client_id: &self.binding.client_id,
            item_number: grant.item_number,
            actor: &grant.actor,
            expected_epoch: grant.epoch,
        };
        let capability: WriterCapability = self.post("api/platform/grants", &request).await?;
        self.store.builtin(BuiltinCommand::Activate {
            session_id: grant.session_id.clone(),
            grant_id: capability.grant_id,
            epoch: capability.epoch,
            token: capability.token,
        })?;
        Ok(())
    }
    pub(crate) async fn revoke_session(&self, session_id: &str) -> Result<()> {
        let BuiltinReply::Grant(Some(grant)) =
            self.store.builtin(BuiltinCommand::Grant(session_id.into()))?
        else {
            return Ok(());
        };
        if grant.lifecycle == "revoked" {
            return Ok(());
        }
        let request_id = format!("{}:revoke", grant.request_id);
        let request = GrantRequest {
            run_id: self.run_id.as_deref(),
            pending: grant.lifecycle == "pending",
            request_id: &request_id,
            client_id: &self.binding.client_id,
            item_number: grant.item_number,
            actor: &grant.actor,
            expected_epoch: grant.epoch,
        };
        let revoked: Revoked = self.post("api/platform/grants/revoke", &request).await?;
        ensure!(revoked.lifecycle == "revoked", "platform did not confirm writer grant revocation");
        self.store.builtin(BuiltinCommand::Revoke {
            session_id: session_id.into(),
            epoch: revoked.epoch,
        })?;
        Ok(())
    }
    pub(crate) async fn reconcile_fence(&self, event: &IngressEvent) -> Result<()> {
        if !matches!(event.kind, EventKind::Assign | EventKind::Unassign | EventKind::Lifecycle) {
            return Ok(());
        }
        let Some(number) = event.work_item_number else {
            return Ok(());
        };
        let BuiltinReply::Grants(grants) = self.store.builtin(BuiltinCommand::Grants)? else {
            unreachable!()
        };
        for grant in grants {
            if grant.item_number == number {
                self.revoke_session(&grant.session_id).await?;
            }
        }
        Ok(())
    }
    pub(crate) async fn revoke_all(&self) -> Result<()> {
        let BuiltinReply::Grants(grants) = self.store.builtin(BuiltinCommand::Grants)? else {
            unreachable!()
        };
        let mut errors = Vec::new();
        for grant in grants {
            if let Err(error) = self.revoke_session(&grant.session_id).await {
                errors.push(format!("session {}: {error:#}", grant.session_id));
            }
        }
        ensure!(errors.is_empty(), "writer grant revocation failed: {}", errors.join("\n"));
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
