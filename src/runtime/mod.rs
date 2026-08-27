#![allow(clippy::large_futures)]
use std::{
    collections::BTreeMap,
    fmt::Write as _,
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
    task::{JoinHandle, JoinSet},
    time::{Duration, MissedTickBehavior},
};
use uuid::Uuid;

use crate::{
    config::{Config, Profile},
    context::{
        self, CanonicalContext, CanonicalObservation, ContextError, ContextPressure,
        RenderedContext,
    },
    github::{
        AppWebhookConfig, CreatedIssueComment, GitHubClient, RepositoryName, WorkItemLocator,
    },
    provider::{ProviderError, ProviderNotification, connect_provider},
    store::{
        AssignmentCandidate, CanonicalObjectState, ContextResetClaim, IngressEvent, ProfileRecord,
        ReactionTarget, RuntimeLease, SchedulerPolicy, StoreActor, TurnClaim,
        WorkItemLifecycleCandidate,
    },
    telemetry::{self, PayloadEvidence, TelemetryGuard},
    tunnel::QuickTunnel,
    webhook::{self, WebhookHeaders},
    worktree::{self, WorktreeRequest},
};

mod ingress;
mod issue_agent;
mod outbox;
mod pr_agent;
mod provider;
mod reconcile;
mod scheduler;
pub mod session_manager;
mod tunnel;

pub use tunnel::probe_public_webhook;

use crate::runtime::ingress::{event_worker, webhook_handler};
use crate::runtime::issue_agent::issue_agent_worker;
use crate::runtime::outbox::drain_one_write;
use crate::runtime::pr_agent::pr_agent_worker;
use crate::runtime::provider::agent_attributions;
use crate::runtime::reconcile::{lease_worker, reconciliation_worker};
use crate::runtime::tunnel::{restore_webhook, start_verified_quick_tunnel};

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
    pub provider: &'static str,
    pub last_error: Option<String>,
}

struct IngressState {
    store: Arc<StoreActor>,
    policy: SchedulerPolicy,
    repository: String,
    handle: String,
    app_actor_node_id: String,
    app_actor_login: String,
    agent_actor_node_ids: Vec<String>,
    agent_attributions: Vec<String>,
    webhook_secret: Arc<Vec<u8>>,
}

struct ReconcileScope {
    work_item_node_id: String,
    work_item_kind: &'static str,
    work_item_number: u64,
    work_item_state: String,
    previous_work_item_state: String,
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
pub async fn serve(config: Config, quick_tunnel: bool, provider_enabled: bool) -> Result<()> {
    let telemetry = TelemetryGuard::install(&config.telemetry)?;
    let store = Arc::new(StoreActor::start(
        config.runtime.database().to_path_buf(),
        config.runtime.backups().to_path_buf(),
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
    let secret = config
        .webhook_secret()
        .with_context(|| "cannot load GitHub webhook secret from configured source")?;
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
        provider: if provider_enabled { "starting" } else { "disabled" },
        last_error: None,
    }));
    let state = Arc::new(IngressState {
        store: Arc::clone(&store),
        policy,
        repository: config.github.repository.clone(),
        handle: config.github.handle.clone(),
        app_actor_node_id: github.identity().actor_node_id.clone(),
        app_actor_login: github.identity().actor_login.clone(),
        agent_actor_node_ids: config
            .profiles
            .iter()
            .filter_map(|profile| profile.github_actor_node_id.clone())
            .collect(),
        agent_attributions: agent_attributions(&config),
        webhook_secret: Arc::new(secret.into_bytes()),
    });
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let ingress =
        spawn_ingress(config.server.ingress, Arc::clone(&state), shutdown_receiver.clone()).await?;
    let health_server =
        spawn_health(config.server.health, Arc::clone(&health), shutdown_receiver.clone()).await?;
    let mut workers = JoinSet::new();
    workers.spawn(event_worker(
        Arc::clone(&store),
        Arc::clone(&github),
        policy,
        shutdown_receiver.clone(),
    ));
    workers.spawn(reconciliation_worker(
        Arc::clone(&store),
        Arc::clone(&github),
        config.clone(),
        Arc::clone(&health),
        shutdown_receiver.clone(),
    ));
    workers.spawn(lease_worker(Arc::clone(&store), Arc::clone(&lease), shutdown_receiver.clone()));

    if provider_enabled {
        let provider = connect_provider(&config.default_provider_config()?).await?;
        health.write().await.provider = "connected";
        let sessions = Arc::new(crate::runtime::session_manager::SessionManager::new());
        workers.spawn(issue_agent_worker(
            Arc::clone(&store),
            Arc::clone(&github),
            config.clone(),
            provider,
            Arc::clone(&sessions),
            Arc::clone(&health),
            shutdown_receiver.clone(),
        ));
        let pr_provider = connect_provider(&config.default_provider_config()?).await?;
        workers.spawn(pr_agent_worker(
            Arc::clone(&store),
            Arc::clone(&github),
            config.clone(),
            pr_provider,
            Arc::clone(&sessions),
            Arc::clone(&health),
            shutdown_receiver.clone(),
        ));
    }

    let local_url = format!("http://{}", config.server.ingress);
    let mut tunnel = None;
    let mut prior_webhook = None;
    if quick_tunnel {
        {
            let mut current = health.write().await;
            current.tunnel = "verifying";
        }
        let (started, public_webhook) = start_verified_quick_tunnel(
            &config,
            &local_url,
            &state.webhook_secret,
            &config.github.repository,
            &github.identity().repository_node_id,
        )
        .await?;
        health.write().await.webhook_url = Some(public_webhook.clone());
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
    let mut shutdown_signal = Box::pin(wait_for_shutdown_signal());
    let mut runtime_failure = None;
    loop {
        tokio::select! {
            result = &mut shutdown_signal => {
                result?;
                break;
            }
            worker = workers.join_next() => {
                let message = match worker {
                    Some(Ok(())) => "a supervised runtime worker stopped unexpectedly".to_owned(),
                    Some(Err(error)) => format!("a supervised runtime worker failed: {error}"),
                    None => "all supervised runtime workers stopped unexpectedly".to_owned(),
                };
                {
                    let mut current = health.write().await;
                    current.ready = false;
                    current.last_error = Some(message.clone());
                }
                runtime_failure = Some(message);
                break;
            }
            _ = tunnel_poll.tick(), if tunnel.is_some() => {
                if tunnel.as_mut().is_some_and(|running| running.has_exited().unwrap_or(true)) {
                    let repair = if let Some(prior) = prior_webhook.take() {
                        restore_webhook(&github, &prior).await.map(|()| prior.url)
                    } else {
                        Ok(String::new())
                    };
                    let mut current = health.write().await;
                    current.tunnel = "unavailable";
                    match repair {
                        Ok(url) => {
                            current.webhook_url = (!url.is_empty()).then_some(url);
                            current.last_error = Some(
                                "Wrangler Quick Tunnel exited; the prior App webhook was restored and reconciliation remains active".into(),
                            );
                        }
                        Err(error) => {
                            current.last_error = Some(format!(
                                "Wrangler Quick Tunnel exited and App webhook repair failed: {error}"
                            ));
                        }
                    }
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
    let workers_stopped = tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(worker) = workers.join_next().await {
            if let Err(error) = worker {
                tracing::error!(%error, "runtime worker did not stop cleanly");
            }
        }
    })
    .await
    .is_ok();
    if !workers_stopped {
        tracing::error!("runtime workers exceeded the shutdown deadline; aborting them");
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
    let outbox_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < outbox_deadline {
        let status = store.runtime_status()?;
        if status.pending_writes == 0 && status.uncertain_writes == 0 {
            break;
        }
        drain_one_write(&store, &github).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), ingress).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), health_server).await;
    drop(store);
    telemetry.shutdown()?;
    if let Some(error) = runtime_failure {
        bail!(error);
    }
    Ok(())
}

async fn wait_for_shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .context("cannot listen for SIGTERM")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => {
                result.context("cannot listen for Ctrl-C")?;
            }
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await.context("cannot listen for Ctrl-C")?;
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
