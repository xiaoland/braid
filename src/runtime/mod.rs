#![allow(clippy::large_futures)]
mod builtin;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::{Context as _, Result, bail};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use tokio::{
    net::TcpListener,
    sync::{RwLock, watch},
    task::{JoinHandle, JoinSet},
    time::Duration,
};
use uuid::Uuid;

use crate::{
    config::Config,
    github::{GitHubClient, RepositoryName},
    store::{RuntimeLease, SchedulerPolicy, StoreActor},
    telemetry::TelemetryGuard,
};

use crate::group::{GroupKind, GroupSpec, agent_group_worker};
use crate::health::HealthSnapshot;
use crate::outbox::{drain_one_write, outbox_worker};
use crate::producer::LEASE_TTL_SECONDS;
use crate::producer::{
    IngressState, lease_worker, mention_worker, reconciliation_worker, webhook_handler,
};
use crate::queue::queue_worker;

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

pub(crate) fn initial_health(config: &Config, provider_enabled: bool) -> HealthSnapshot {
    if config.is_builtin() {
        return builtin::initial_health(config, provider_enabled);
    }
    HealthSnapshot {
        ready: false,
        ingress: config.server.ingress.to_string(),
        repository: config.github_repositories().join(", "),
        tunnel: "disabled",
        webhook_url: None,
        webhook_sync: "disabled",
        tunnel_verified_at: None,
        tunnel_error: None,
        reconciliation: "starting",
        provider: if provider_enabled { "starting" } else { "disabled" },
        last_error: None,
    }
}

pub async fn serve(config: Config, quick_tunnel: bool, provider_enabled: bool) -> Result<()> {
    if config.is_builtin() && quick_tunnel {
        bail!("built-in upstream does not use a GitHub webhook tunnel");
    }
    let telemetry = TelemetryGuard::install(&config.telemetry, config.instance_key())?;
    let generation = Uuid::now_v7().to_string();
    crate::telemetry::set_runtime_generation(Some(generation.clone()));
    let health = Arc::new(RwLock::new(initial_health(&config, provider_enabled)));
    let (stop, shutdown) = watch::channel(false);
    let mut runtime = Box::pin(async move {
        if config.is_builtin() {
            serve_managed(config, provider_enabled, shutdown, health).await
        } else {
            serve_github_managed(config, quick_tunnel, provider_enabled, shutdown, health).await
        }
    });
    let result = tokio::select! {
        result=&mut runtime=>result,
        signal=wait_for_shutdown_signal()=>{let _=stop.send(true);signal.and(runtime.await)}
    };
    tracing::info!(name="braid.runtime.stop",braid.runtime.generation=%generation,braid.runtime.stop_confirmed=result.is_ok(),braid.runtime.error=result.as_ref().err().map(|error|format!("{error:#}")).as_deref().unwrap_or(""),"standalone runtime ended");
    crate::telemetry::set_runtime_generation(None);
    match (result, telemetry.shutdown()) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error.into()),
        (Err(error), Err(exporter)) => {
            Err(error.context(format!("telemetry shutdown also failed: {exporter}")))
        }
    }
}

pub(crate) async fn serve_managed(
    config: Config,
    provider_enabled: bool,
    stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<()> {
    if config.is_builtin() {
        builtin::serve_managed(config, provider_enabled, stop, health).await
    } else {
        serve_github_managed(config, false, provider_enabled, stop, health).await
    }
}

#[allow(clippy::too_many_lines)]
async fn serve_github_managed(
    config: Config,
    quick_tunnel: bool,
    provider_enabled: bool,
    mut external_stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<()> {
    if *external_stop.borrow() {
        return Ok(());
    }
    let mut starting = initial_health(&config, provider_enabled);
    if quick_tunnel {
        starting.tunnel = "verifying";
        starting.webhook_sync = "pending";
    }
    *health.write().await = starting;
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
    let connection = rusqlite::Connection::open_with_flags(
        config.runtime.database(),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let local_space: i64 =
        connection.query_row("SELECT COUNT(*) FROM local_space", [], |row| row.get(0))?;
    if local_space != 0 {
        bail!(
            "this database contains legacy collaboration objects; import them into the built-in platform and use a separate Harness ledger for GitHub"
        );
    }
    drop(connection);
    let owner_id = Uuid::now_v7().to_string();
    let lease = store.acquire_runtime_lease("runtime".into(), owner_id, LEASE_TTL_SECONDS)?;
    let lease = Arc::new(StdMutex::new(lease));
    let _lease_guard = RuntimeLeaseGuard { store: Arc::clone(&store), lease: Arc::clone(&lease) };
    store.recover_writes()?;

    let repositories = config
        .github_repositories()
        .into_iter()
        .map(str::parse::<RepositoryName>)
        .collect::<Result<Vec<_>, _>>()?;
    let mut github_client_map: HashMap<String, Arc<GitHubClient>> = HashMap::new();
    let rate_limits = Arc::new(crate::github::RateLimits::default());
    for repository in &repositories {
        let client = if let Some(existing) = github_client_map.values().next() {
            Box::pin(existing.for_repository(repository)).await?
        } else {
            Box::pin(GitHubClient::connect_shared(
                config.github()?,
                repository,
                Arc::clone(&rate_limits),
            ))
            .await?
        };
        if !client
            .identity()
            .permissions
            .get("issues")
            .is_some_and(|permission| permission == "write" || permission == "admin")
        {
            bail!(
                "GitHub repository {repository} requires Issues: write for Braid-owned reactions"
            );
        }
        github_client_map.insert(repository.name_with_owner(), Arc::new(client));
    }
    let github =
        github_client_map.get(config.repository()).expect("primary repository client").clone();
    let github_clients = Arc::new(github_client_map);
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
    let state = Arc::new(IngressState {
        store: Arc::clone(&store),
        policy,
        repositories: config.github_repositories().into_iter().map(str::to_owned).collect(),
        handle: config.github()?.handle.clone(),
        app_actor_node_id: github.identity().actor_node_id.clone(),
        app_actor_login: github.identity().actor_login.clone(),
        agent_actor_node_ids: config
            .profiles
            .iter()
            .filter_map(|profile| profile.github_actor_node_id.clone())
            .collect(),
        webhook_secret: Arc::new(secret.into_bytes()),
    });
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let ingress =
        spawn_ingress(config.server.ingress, Arc::clone(&state), shutdown_receiver.clone()).await?;
    let health_server =
        spawn_health(config.server.health, Arc::clone(&health), shutdown_receiver.clone()).await?;
    let mut workers = JoinSet::new();
    workers.spawn(worker_result(queue_worker(Arc::clone(&store), shutdown_receiver.clone())));
    workers.spawn(worker_result(mention_worker(
        Arc::clone(&store),
        Arc::clone(&github_clients),
        policy,
        shutdown_receiver.clone(),
    )));
    workers.spawn(worker_result(outbox_worker(
        Arc::clone(&store),
        Arc::clone(&github_clients),
        config.clone(),
        lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone(),
        shutdown_receiver.clone(),
    )));
    workers.spawn(worker_result(reconciliation_worker(
        Arc::clone(&store),
        Arc::clone(&github_clients),
        config.clone(),
        Arc::clone(&health),
        shutdown_receiver.clone(),
    )));
    workers.spawn(worker_result(lease_worker(
        Arc::clone(&store),
        Arc::clone(&lease),
        shutdown_receiver.clone(),
    )));

    // Startup can fail after workers have resumed persisted executions. Route
    // those failures through the same bounded shutdown as a running service.
    let startup: Result<()> = (|| {
        if provider_enabled {
            let factories = crate::provider::session_factories(&config)?;
            let mut spec_scopes = Vec::new();
            for repository in config.github_repositories() {
                for kind in [GroupKind::Issue, GroupKind::Pr] {
                    if !config.profiles_for_repository(repository, kind.as_str()).is_empty() {
                        spec_scopes.push((repository, kind));
                    }
                }
            }
            let specs = spec_scopes
                .into_iter()
                .map(|(repository, kind)| GroupSpec::new(kind, &config, &store, repository))
                .collect::<Result<Vec<_>>>()?;
            let (reports, receiver) = tokio::sync::mpsc::channel(8);
            workers.spawn(worker_result(crate::health::provider_health_worker(
                Arc::clone(&health),
                specs.len(),
                receiver,
                shutdown_receiver.clone(),
            )));
            for spec in specs {
                let factory = factories
                    .get(spec.profile_id())
                    .context("Profile session factory missing")?
                    .clone();
                workers.spawn(agent_group_worker(
                    Arc::clone(&store),
                    github_clients
                        .get(&spec.repository)
                        .context("repository GitHub client missing")?
                        .clone(),
                    config.clone(),
                    spec,
                    factory,
                    reports.clone(),
                    shutdown_receiver.clone(),
                ));
            }
            drop(reports);
        }

        Ok(())
    })();

    let mut tunnel = JoinSet::new();
    let mut failures = Vec::new();
    if let Err(error) = startup {
        failures.push(format!("runtime startup failed: {error:#}"));
    } else {
        if quick_tunnel {
            tunnel.spawn(crate::tunnel::supervise(
                config.clone(),
                github,
                Arc::clone(&state.webhook_secret),
                Arc::clone(&health),
                shutdown_receiver,
            ));
        }
        health.write().await.ready = true;
        tracing::info!(repositories = %config.github_repositories().join(", "), "Braid transport runtime started");
        loop {
            tokio::select! {
                _ = external_stop.changed() => break,
                worker = workers.join_next() => {
                    let message = match worker {
                        Some(Ok(Ok(()))) => "a supervised runtime worker stopped unexpectedly".to_owned(),
                        Some(Ok(Err(error))) => format!("a supervised runtime worker failed: {error:#}"),
                        Some(Err(error)) => format!("a supervised runtime worker failed: {error}"),
                        None => "all supervised runtime workers stopped unexpectedly".to_owned(),
                    };
                    {
                        let mut current = health.write().await;
                        current.ready = false;
                        current.last_error = Some(message.clone());
                    }
                    failures.push(message);
                    break;
                }
                result = tunnel.join_next(), if !tunnel.is_empty() => {
                    let reason = match result {
                        Some(Ok(Err(error))) => format!("Quick Tunnel supervisor failed: {error:#}"),
                        Some(Err(error)) => format!("Quick Tunnel supervisor failed: {error}"),
                        _ => "Quick Tunnel supervisor stopped unexpectedly".into(),
                    };
                    tracing::error!(%reason, "Quick Tunnel can no longer confirm its stop");
                    let mut current = health.write().await;
                    current.tunnel = "stop_unconfirmed";
                    current.webhook_sync = "blocked";
                    current.tunnel_error = Some(reason.clone());
                    failures.push(reason);
                }
            }
        }
    }

    health.write().await.ready = false;
    let _ = shutdown_sender.send(true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let (workers_stopped, tunnel_stopped) = tokio::join!(
        tokio::time::timeout_at(deadline, async {
            while let Some(worker) = workers.join_next().await {
                match worker {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        failures
                            .push(format!("runtime worker did not confirm shutdown: {error:#}"));
                    }
                    Err(error) => {
                        failures.push(format!("runtime worker did not stop cleanly: {error}"));
                    }
                }
            }
        }),
        tokio::time::timeout_at(deadline, tunnel.join_next()),
    );
    match tunnel_stopped {
        Ok(Some(Ok(Err(error)))) => failures.push(format!("{error:#}")),
        Ok(Some(Err(error))) => {
            failures.push(format!("Quick Tunnel did not stop cleanly: {error}"));
        }
        Ok(_) => {}
        Err(_) => {
            failures
                .push("Quick Tunnel exceeded its shutdown deadline; stop is unconfirmed".into());
            tunnel.abort_all();
            while tunnel.join_next().await.is_some() {}
        }
    }
    if workers_stopped.is_err() {
        let reason =
            "runtime workers exceeded the shutdown deadline; execution stop is unconfirmed";
        failures.push(reason.into());
        tracing::error!("{reason}");
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
    let outbox_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < outbox_deadline {
        let status = store.runtime_status()?;
        if status.pending_writes == 0 && status.uncertain_writes == 0 {
            break;
        }
        let owner = lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        drain_one_write(&store, &github_clients, &config, &owner).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), ingress).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), health_server).await;
    drop(store);
    if !failures.is_empty() {
        bail!(failures.join("\n"));
    }
    Ok(())
}

pub(crate) async fn worker_result(future: impl std::future::Future<Output = ()>) -> Result<()> {
    future.await;
    Ok(())
}

pub(crate) async fn wait_for_shutdown_signal() -> Result<()> {
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

pub(crate) async fn spawn_health(
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
    let mut snapshot = state.read().await.clone();
    snapshot.ready &= matches!(snapshot.tunnel, "disabled" | "connected");
    let status = if snapshot.ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (status, axum::Json(snapshot))
}

pub(crate) async fn builtin_run_managed(
    config: Config,
    request: crate::run::RunRequest,
    store: Arc<StoreActor>,
    stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<crate::run::RunResult> {
    builtin::finite_managed(config, request, store, stop, health).await
}
