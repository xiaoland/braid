use std::{
    collections::BTreeMap,
    env,
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{Context as _, Result, bail};
use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::{
    net::TcpListener,
    sync::{RwLock, watch},
    task::JoinHandle,
    time::{Duration, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{
    config::Config,
    context::{self, CanonicalContext, CanonicalObservation},
    github::{AppWebhookConfig, GitHubClient, RepositoryName, WorkItemLocator},
    store::{
        CanonicalObjectState, IngressEvent, ReactionTarget, RuntimeLease, SchedulerPolicy,
        StoreActor,
    },
    telemetry::{self, PayloadEvidence, TelemetryGuard},
    tunnel::QuickTunnel,
    webhook::{self, WebhookHeaders},
};

type HmacSha256 = Hmac<Sha256>;
const LEASE_TTL_SECONDS: u64 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct HealthSnapshot {
    pub ready: bool,
    pub ingress: String,
    pub repository: String,
    pub tunnel: &'static str,
    pub webhook_url: Option<String>,
    pub reconciliation: &'static str,
    pub last_error: Option<String>,
}

struct IngressState {
    store: Arc<StoreActor>,
    policy: SchedulerPolicy,
    repository: String,
    handle: String,
    app_actor_node_id: String,
    app_actor_login: String,
    webhook_secret: Arc<Vec<u8>>,
}

struct ReconcileScope {
    work_item_node_id: String,
    work_item_kind: &'static str,
    work_item_number: u64,
    work_item_state: String,
    repository_node_id: String,
    repository: String,
}

struct RuntimeLeaseGuard {
    store: Arc<StoreActor>,
    lease: Arc<StdMutex<RuntimeLease>>,
}

impl Drop for RuntimeLeaseGuard {
    fn drop(&mut self) {
        let lease = self.lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if let Err(error) = self.store.release_runtime_lease(lease) {
            tracing::error!(%error, "cannot release runtime owner lease");
        }
    }
}

#[allow(clippy::too_many_lines)]
pub async fn serve(config: Config, quick_tunnel: bool) -> Result<()> {
    let telemetry = TelemetryGuard::install(&config.telemetry)?;
    let store = Arc::new(StoreActor::start(
        config.runtime.database.clone(),
        config.runtime.backups.clone(),
    )?);
    let plan = store.plan()?;
    if !plan.pending.is_empty() {
        if config.runtime.auto_migrate {
            store.apply()?;
        } else {
            bail!(
                "database schema {}/{} is not ready; run `braid migrate apply --config <PATH>`",
                plan.current_schema,
                plan.supported_schema
            );
        }
    }
    let owner_id = Uuid::now_v7().to_string();
    let lease = store.acquire_runtime_lease("runtime".into(), owner_id, LEASE_TTL_SECONDS)?;
    let lease = Arc::new(StdMutex::new(lease));
    let _lease_guard = RuntimeLeaseGuard { store: Arc::clone(&store), lease: Arc::clone(&lease) };
    store.recover_writes()?;

    let repository = config.github.repository.parse::<RepositoryName>()?;
    let github = Arc::new(GitHubClient::connect(&config.github, &repository).await?);
    if !github
        .identity()
        .permissions
        .get("issues")
        .is_some_and(|permission| permission == "write" || permission == "admin")
    {
        bail!("Slice 2 requires GitHub App Issues: write for Braid-owned reactions");
    }
    let secret = env::var(&config.github.webhook_secret_environment).with_context(|| {
        format!(
            "environment variable {} must contain the GitHub webhook secret",
            config.github.webhook_secret_environment
        )
    })?;
    if secret.is_empty() {
        bail!("GitHub webhook secret must not be empty");
    }
    let policy = SchedulerPolicy {
        quiet_seconds: config.scheduler.quiet_seconds,
        event_threshold: config.scheduler.event_threshold,
    };
    let health = Arc::new(RwLock::new(HealthSnapshot {
        ready: false,
        ingress: config.server.ingress.to_string(),
        repository: config.github.repository.clone(),
        tunnel: if quick_tunnel { "starting" } else { "disabled" },
        webhook_url: None,
        reconciliation: "starting",
        last_error: None,
    }));
    let state = Arc::new(IngressState {
        store: Arc::clone(&store),
        policy,
        repository: config.github.repository.clone(),
        handle: config.github.handle.clone(),
        app_actor_node_id: github.identity().actor_node_id.clone(),
        app_actor_login: github.identity().actor_login.clone(),
        webhook_secret: Arc::new(secret.into_bytes()),
    });
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let ingress =
        spawn_ingress(config.server.ingress, Arc::clone(&state), shutdown_receiver.clone()).await?;
    let health_server =
        spawn_health(config.server.health, Arc::clone(&health), shutdown_receiver.clone()).await?;
    let workers = vec![
        tokio::spawn(event_worker(
            Arc::clone(&store),
            Arc::clone(&github),
            policy,
            shutdown_receiver.clone(),
        )),
        tokio::spawn(reconciliation_worker(
            Arc::clone(&store),
            Arc::clone(&github),
            config.clone(),
            Arc::clone(&health),
            shutdown_receiver.clone(),
        )),
        tokio::spawn(lease_worker(
            Arc::clone(&store),
            Arc::clone(&lease),
            shutdown_receiver.clone(),
        )),
    ];

    let local_url = format!("http://{}", config.server.ingress);
    let mut tunnel = None;
    let mut prior_webhook = None;
    if quick_tunnel {
        let started = QuickTunnel::start(&config.tools.wrangler, &local_url).await?;
        let public_webhook = format!("{}/webhook", started.url);
        {
            let mut current = health.write().await;
            current.tunnel = "verifying";
            current.webhook_url = Some(public_webhook.clone());
        }
        signed_public_probe(
            &public_webhook,
            &state.webhook_secret,
            &config.github.repository,
            &github.identity().repository_node_id,
        )
        .await?;
        let prior = github.app_webhook_config().await?;
        github
            .update_app_webhook(
                &public_webhook,
                Some(
                    std::str::from_utf8(&state.webhook_secret)
                        .context("webhook secret is UTF-8")?,
                ),
            )
            .await?;
        {
            let mut current = health.write().await;
            current.tunnel = "connected";
            current.webhook_url = Some(public_webhook);
        }
        prior_webhook = Some(prior);
        tunnel = Some(started);
    }
    health.write().await.ready = true;
    tracing::info!(repository = %config.github.repository, "Braid transport runtime is ready");

    let mut tunnel_poll = tokio::time::interval(Duration::from_secs(2));
    tunnel_poll.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("cannot listen for shutdown signal")?;
                break;
            }
            _ = tunnel_poll.tick(), if tunnel.is_some() => {
                if tunnel.as_mut().is_some_and(|running| running.has_exited().unwrap_or(true)) {
                    let mut current = health.write().await;
                    current.tunnel = "unavailable";
                    current.last_error = Some("Wrangler Quick Tunnel exited; reconciliation remains active".into());
                    tunnel = None;
                }
            }
        }
    }

    health.write().await.ready = false;
    if let Some(prior) = &prior_webhook
        && let Err(error) = restore_webhook(&github, prior).await
    {
        tracing::error!(%error, "cannot restore prior GitHub App webhook");
    }
    if let Some(tunnel) = tunnel
        && let Err(error) = tunnel.stop().await
    {
        tracing::error!(%error, "cannot stop Wrangler Quick Tunnel");
    }
    let _ = shutdown_sender.send(true);
    for worker in workers {
        if let Err(error) = worker.await {
            tracing::error!(%error, "runtime worker did not stop cleanly");
        }
    }
    let _ = ingress.await;
    let _ = health_server.await;
    drop(store);
    telemetry.shutdown()?;
    Ok(())
}

async fn spawn_ingress(
    address: std::net::SocketAddr,
    state: Arc<IngressState>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<JoinHandle<()>> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot bind webhook ingress {address}"))?;
    let app = Router::new().route("/webhook", post(webhook_handler)).with_state(state);
    Ok(tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await
        {
            tracing::error!(%error, "webhook ingress stopped");
        }
    }))
}

async fn spawn_health(
    address: std::net::SocketAddr,
    state: Arc<RwLock<HealthSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<JoinHandle<()>> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot bind health endpoint {address}"))?;
    let app = Router::new().route("/healthz", get(health_handler)).with_state(state);
    Ok(tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown.changed().await;
            })
            .await
        {
            tracing::error!(%error, "health endpoint stopped");
        }
    }))
}

async fn health_handler(State(state): State<Arc<RwLock<HealthSnapshot>>>) -> impl IntoResponse {
    let snapshot = state.read().await.clone();
    let status = if snapshot.ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (status, axum::Json(snapshot))
}

async fn webhook_handler(
    State(state): State<Arc<IngressState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &'static str| headers.get(name).and_then(|value| value.to_str().ok());
    let parsed = webhook::parse_verified(
        WebhookHeaders {
            signature: header("x-hub-signature-256"),
            delivery: header("x-github-delivery"),
            event: header("x-github-event"),
        },
        &body,
        &state.webhook_secret,
        &state.repository,
        &state.handle,
        &state.app_actor_node_id,
        &state.app_actor_login,
    );
    let event = match parsed {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(%error, "rejected GitHub webhook");
            let status = if matches!(
                error,
                webhook::WebhookError::MissingSignature | webhook::WebhookError::InvalidSignature
            ) {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };
            return (status, error.to_string()).into_response();
        }
    };
    let span = tracing::info_span!(
        "github.delivery",
        github.delivery = %event.delivery_guid,
        github.event = %event.event_name,
        github.action = event.action.as_deref().unwrap_or("")
    );
    let _entered = span.enter();
    telemetry::emit_payload_event(&PayloadEvidence {
        github_body: "",
        github_summary: &event.reference,
        credential: "",
        provider_transcript: "",
        webhook_payload: std::str::from_utf8(&body).unwrap_or("<non-UTF-8 webhook>"),
        local_path: "",
    });
    match state.store.ingest_event(event, state.policy) {
        Ok(result) => {
            let status = if result.duplicate { StatusCode::OK } else { StatusCode::ACCEPTED };
            (status, axum::Json(result)).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "cannot durably ingest GitHub webhook");
            (StatusCode::SERVICE_UNAVAILABLE, "durable ingest unavailable").into_response()
        }
    }
}

async fn event_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    policy: SchedulerPolicy,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                if let Err(error) = store.advance_scheduler() {
                    tracing::error!(%error, "cannot advance scheduler");
                }
                match store.mention_candidates(16) {
                    Ok(candidates) => {
                        for candidate in candidates {
                            match github.repository_permission(&candidate.actor_login).await {
                                Ok(role) => {
                                    let trusted = matches!(role.to_ascii_lowercase().as_str(), "maintain" | "admin");
                                    if let Err(error) = store.resolve_mention(candidate.event_id, trusted, policy) {
                                        tracing::error!(%error, "cannot resolve mention authority");
                                    }
                                }
                                Err(error) => tracing::warn!(%error, actor = %candidate.actor_login, "mention authority remains unresolved"),
                            }
                        }
                    }
                    Err(error) => tracing::error!(%error, "cannot load mention candidates"),
                }
                drain_one_write(&store, &github).await;
            }
        }
    }
}

async fn drain_one_write(store: &StoreActor, github: &GitHubClient) {
    let write = match store.claim_github_write() {
        Ok(write) => write,
        Err(error) => {
            tracing::error!(%error, "cannot claim GitHub write");
            return;
        }
    };
    let Some(write) = write else { return };
    if write.repository != github.identity().repository {
        let _ = store.finish_github_write(
            write.intent_id,
            "rejected",
            None,
            Some("write repository does not match connected installation".into()),
        );
        return;
    }
    let result = if write.operation == "reaction_add" {
        github.add_reaction(&write.target_kind, &write.target_database_id, &write.content).await
    } else {
        bail_unknown_write(&write.operation)
    };
    match result {
        Ok(remote_id) => {
            if let Err(error) = store.finish_github_write(
                write.intent_id,
                "applied",
                Some(remote_id.to_string()),
                None,
            ) {
                tracing::error!(%error, "cannot acknowledge GitHub write");
            }
        }
        Err(error) => {
            let lifecycle = if error.is_unavailable() { "uncertain" } else { "rejected" };
            if let Err(store_error) =
                store.finish_github_write(write.intent_id, lifecycle, None, Some(error.to_string()))
            {
                tracing::error!(%store_error, "cannot record GitHub write failure");
            }
        }
    }
}

fn bail_unknown_write(operation: &str) -> Result<u64, crate::github::GitHubError> {
    Err(crate::github::GitHubError::GraphQl(format!("unsupported outbox operation {operation:?}")))
}

async fn lease_worker(
    store: Arc<StoreActor>,
    lease: Arc<StdMutex<RuntimeLease>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(10));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                let current = lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
                match store.renew_runtime_lease(current, LEASE_TTL_SECONDS) {
                    Ok(renewed) => {
                        *lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = renewed;
                    }
                    Err(error) => {
                        tracing::error!(%error, "runtime owner lease was lost");
                        break;
                    }
                }
            }
        }
    }
}

async fn reconciliation_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    config: Config,
    health: Arc<RwLock<HealthSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick =
        tokio::time::interval(Duration::from_secs(config.scheduler.reconciliation_seconds));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                match Box::pin(reconcile_once(&store, &github, &config)).await {
                    Ok(changes) => {
                        let mut current = health.write().await;
                        current.reconciliation = "current";
                        if changes > 0 {
                            tracing::info!(changes, "canonical GitHub reconciliation observed changes");
                        }
                    }
                    Err(error) => {
                        let mut current = health.write().await;
                        current.reconciliation = "unavailable";
                        current.last_error = Some(error.to_string());
                        tracing::error!(%error, "canonical GitHub reconciliation failed");
                    }
                }
            }
        }
    }
}

async fn reconcile_once(
    store: &StoreActor,
    github: &GitHubClient,
    config: &Config,
) -> Result<usize> {
    let run = store.begin_reconciliation(github.identity().repository_node_id.clone())?;
    let result = Box::pin(reconcile_work_items(store, github, config)).await;
    match &result {
        Ok((work_items, changes)) => {
            store.finish_reconciliation(run, "completed", *work_items, *changes, None)?;
        }
        Err(error) => {
            store.finish_reconciliation(run, "failed", 0, 0, Some(error.to_string()))?;
        }
    }
    result.map(|(_, changes)| changes)
}

async fn reconcile_work_items(
    store: &StoreActor,
    github: &GitHubClient,
    config: &Config,
) -> Result<(usize, usize)> {
    let tracked = store.tracked_work_items()?;
    let mut changes = 0;
    for work_item in &tracked {
        let repository = work_item.repository.parse::<RepositoryName>()?;
        let scoped;
        let client = if repository == config.github.repository.parse::<RepositoryName>()? {
            github
        } else {
            scoped = github.for_repository(&repository).await?;
            &scoped
        };
        let locator = WorkItemLocator { repository, number: work_item.number };
        let mut canonical = if work_item.kind == "issue" {
            CanonicalContext::Issue(context::materialize_issue(client, &locator, 100).await?)
        } else if work_item.kind == "pr" {
            CanonicalContext::PullRequest(
                context::materialize_pull_request(client, &locator, 100).await?,
            )
        } else {
            continue;
        };
        let prior = store
            .canonical_objects(work_item.node_id.clone())?
            .into_iter()
            .filter(|object| {
                matches!(
                    object.object_kind.as_str(),
                    "issue"
                        | "pr"
                        | "issue_comment"
                        | "pr_comment"
                        | "review"
                        | "review_comment"
                        | "review_thread"
                )
            })
            .map(|object| (object.node_id.clone(), object))
            .collect::<BTreeMap<_, _>>();
        let scope = match &canonical {
            CanonicalContext::Issue(issue) => ReconcileScope {
                work_item_node_id: issue.node_id.clone(),
                work_item_kind: "issue",
                work_item_number: issue.number,
                work_item_state: issue.state.clone(),
                repository_node_id: issue.repository_node_id.clone(),
                repository: issue.repository.clone(),
            },
            CanonicalContext::PullRequest(pull_request) => ReconcileScope {
                work_item_node_id: pull_request.node_id.clone(),
                work_item_kind: "pr",
                work_item_number: pull_request.number,
                work_item_state: pull_request.state.clone(),
                repository_node_id: pull_request.repository_node_id.clone(),
                repository: pull_request.repository.clone(),
            },
        };
        let observations = context::canonical_observations(&canonical);
        changes += reconcile_observations(store, github, config, &scope, &prior, &observations)?;
        context::reconcile_local_state(&mut canonical, store)?;
        let profile = if work_item.kind == "pr" {
            config.profile(&config.profile_selection.default_pr_profile)?
        } else {
            config
                .profiles
                .iter()
                .find(|profile| profile.has_tag("issue"))
                .context("configuration has no Issue Profile")?
        };
        let rendered = context::render(
            &canonical,
            profile.github_context_soft_ratio,
            profile.github_context_hard_bytes,
        )?;
        context::record_context_revision(&canonical, &rendered, store)?;
    }
    Ok((tracked.len(), changes))
}

fn reconcile_observations(
    store: &StoreActor,
    github: &GitHubClient,
    config: &Config,
    scope: &ReconcileScope,
    prior: &BTreeMap<String, CanonicalObjectState>,
    current: &[CanonicalObservation],
) -> Result<usize> {
    let current_map = current
        .iter()
        .map(|observation| (observation.object_node_id.as_str(), observation))
        .collect::<BTreeMap<_, _>>();
    let policy = SchedulerPolicy {
        quiet_seconds: config.scheduler.quiet_seconds,
        event_threshold: config.scheduler.event_threshold,
    };
    let mut changes = 0;
    for observation in current {
        let previous = prior.get(&observation.object_node_id);
        if previous.is_some_and(|previous| {
            previous.version == observation.version
                && previous.digest == observation.digest
                && previous.lifecycle == observation.lifecycle
        }) {
            continue;
        }
        let (action, classification) = reconciled_change(previous, observation);
        let external =
            observation.author_node_id.as_deref() != Some(github.identity().actor_node_id.as_str());
        let event = reconciled_event(observation, action, classification, external, config);
        if store.ingest_event(event, policy)?.event_id.is_some() {
            changes += 1;
        }
    }
    for previous in prior.values() {
        if current_map.contains_key(previous.node_id.as_str()) || previous.lifecycle == "deleted" {
            continue;
        }
        let observation = CanonicalObservation {
            work_item_node_id: scope.work_item_node_id.clone(),
            work_item_kind: scope.work_item_kind,
            work_item_number: scope.work_item_number,
            work_item_state: scope.work_item_state.clone(),
            repository_node_id: scope.repository_node_id.clone(),
            repository: scope.repository.clone(),
            object_node_id: previous.node_id.clone(),
            database_id: previous.database_id.clone().unwrap_or_default(),
            object_kind: if previous.object_kind == "review_comment" {
                "review_comment"
            } else if scope.work_item_kind == "pr" {
                "pr_comment"
            } else {
                "issue_comment"
            },
            version: format!("{}:deleted", previous.version),
            digest: previous.digest.clone(),
            lifecycle: "deleted",
            author_node_id: previous.author_node_id.clone(),
            author_login: previous.author_login.clone(),
            body: None,
        };
        let event = reconciled_event(&observation, "deleted", "hard_invalidation", true, config);
        if store.ingest_event(event, policy)?.event_id.is_some() {
            changes += 1;
        }
    }
    Ok(changes)
}

fn reconciled_change(
    previous: Option<&CanonicalObjectState>,
    observation: &CanonicalObservation,
) -> (&'static str, &'static str) {
    let restored = previous.is_some_and(|previous| {
        previous.lifecycle == "minimized" && observation.lifecycle == "active"
    });
    match observation.object_kind {
        "issue" | "pr" if previous.is_none() => ("observed", "no_wake"),
        "review" if previous.is_none() => ("submitted", "wake"),
        "review" if observation.lifecycle == "dismissed" => ("dismissed", "hard_invalidation"),
        "review_thread" if observation.lifecycle == "resolved" => ("resolved", "hard_invalidation"),
        "review_thread" if previous.is_none_or(|previous| previous.lifecycle == "resolved") => {
            ("unresolved", "wake")
        }
        _ if previous.is_none() => ("created", "wake"),
        _ if restored => ("unminimized", "wake"),
        _ if observation.lifecycle == "minimized" => ("minimized", "hard_invalidation"),
        _ => ("edited", "hard_invalidation"),
    }
}

fn reconciled_event(
    observation: &CanonicalObservation,
    action: &'static str,
    classification: &'static str,
    external: bool,
    config: &Config,
) -> IngressEvent {
    let event_name = match observation.object_kind {
        "issue" => "issues",
        "pr" => "pull_request",
        "review" => "pull_request_review",
        "review_comment" => "pull_request_review_comment",
        "review_thread" => "pull_request_review_thread",
        _ => "issue_comment",
    };
    let mention_candidate = external
        && matches!(action, "created" | "edited" | "unminimized")
        && observation
            .body
            .as_deref()
            .is_some_and(|body| webhook::has_visible_mention(body, &config.github.handle));
    let reaction_target = (external && action == "created").then(|| ReactionTarget {
        kind: if observation.object_kind == "review_comment" {
            "review_comment"
        } else {
            "issue_comment"
        },
        database_id: observation.database_id.clone(),
    });
    let raw = serde_json::to_vec(&serde_json::json!({
        "source":"reconciliation",
        "action":action,
        "object":observation.object_node_id,
        "version":observation.version,
    }))
    .expect("reconciliation evidence serializes");
    let delivery_guid = format!(
        "reconcile-{}",
        &hex::encode(Sha256::digest(
            format!(
                "{}\0{}\0{}\0{}",
                observation.object_node_id, observation.version, observation.digest, action
            )
            .as_bytes()
        ))[..32]
    );
    IngressEvent {
        delivery_guid,
        event_name: event_name.into(),
        action: Some(action.into()),
        repository_node_id: observation.repository_node_id.clone(),
        repository: observation.repository.clone(),
        work_item_node_id: Some(observation.work_item_node_id.clone()),
        work_item_kind: Some(observation.work_item_kind),
        work_item_number: Some(observation.work_item_number),
        work_item_state: Some(observation.work_item_state.clone()),
        object_node_id: Some(observation.object_node_id.clone()),
        object_version: Some(observation.version.clone()),
        object_digest: Some(observation.digest.clone()),
        actor_node_id: observation.author_node_id.clone(),
        actor_login: observation.author_login.clone(),
        classification: if external { classification } else { "agent_origin" },
        origin: if external { "reconciliation" } else { "agent" },
        reference: format!(
            "GitHub {} {}#{} object {} at {}",
            observation.work_item_kind,
            observation.repository,
            observation.work_item_number,
            observation.object_node_id,
            observation.version
        ),
        mention_candidate,
        reaction_target,
        known: true,
        raw_payload: raw,
    }
}

async fn signed_public_probe(
    url: &str,
    secret: &[u8],
    repository: &str,
    repository_node_id: &str,
) -> Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({
        "zen":"Braid signed public tunnel probe",
        "repository":{"full_name":repository,"node_id":repository_node_id}
    }))?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .tls_backend_rustls()
        .build()
        .context("cannot construct public tunnel probe client")?;
    let mut last = None;
    for _ in 0..6 {
        match client
            .post(url)
            .header("X-Hub-Signature-256", &signature)
            .header("X-GitHub-Delivery", Uuid::now_v7().to_string())
            .header("X-GitHub-Event", "ping")
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => last = Some(format!("HTTP {}", response.status())),
            Err(error) => last = Some(format!("{error:?}")),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    bail!(
        "signed public tunnel probe did not converge: {}",
        last.as_deref().unwrap_or("no response")
    )
}

pub async fn probe_public_webhook(config: &Config, url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("public webhook URL is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(|host| !host.ends_with(".trycloudflare.com"))
        || parsed.path() != "/webhook"
    {
        bail!("public webhook URL must be an HTTPS trycloudflare.com /webhook endpoint");
    }
    let secret = env::var(&config.github.webhook_secret_environment).with_context(|| {
        format!(
            "environment variable {} must contain the GitHub webhook secret",
            config.github.webhook_secret_environment
        )
    })?;
    if secret.is_empty() {
        bail!("GitHub webhook secret must not be empty");
    }
    let repository = config.github.repository.parse::<RepositoryName>()?;
    let github = GitHubClient::connect(&config.github, &repository).await?;
    signed_public_probe(
        url,
        secret.as_bytes(),
        &config.github.repository,
        &github.identity().repository_node_id,
    )
    .await
}

async fn restore_webhook(github: &GitHubClient, prior: &AppWebhookConfig) -> Result<()> {
    github.update_app_webhook(&prior.url, None).await?;
    Ok(())
}
