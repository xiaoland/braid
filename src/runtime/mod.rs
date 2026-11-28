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
    sync::watch,
    task::{Id, JoinHandle, JoinSet},
    time::Duration,
};
use uuid::Uuid;

use crate::{
    config::Config,
    github::{GitHubClient, RepositoryName},
    store::{RuntimeLease, SchedulerPolicy, StoreActor},
    telemetry::TelemetryGuard,
};

use crate::group::{
    GroupKind, GroupSpec, agent_group_worker,
    availability::{ProviderAvailability, ProviderReport, track_provider_availability},
};
use crate::health::{Health, HealthSnapshot};
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
    let health = Health::new(initial_health(&config, provider_enabled));
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
    health: Health,
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
    health: Health,
) -> Result<()> {
    if *external_stop.borrow() {
        return Ok(());
    }
    let mut starting = initial_health(&config, provider_enabled);
    if quick_tunnel {
        starting.tunnel = "verifying";
        starting.webhook_sync = "pending";
    }
    health.update(|current| *current = starting);
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
            tracing::warn!(%repository, "GitHub Issues write permission is unavailable; Braid-owned reactions may be rejected");
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
        spawn_health(config.server.health, health.clone(), shutdown_receiver.clone()).await;
    let mut workers = JoinSet::new();
    let (group_stop, group_shutdown) = watch::channel(false);
    let (reports, report_receiver) = tokio::sync::mpsc::channel(8);
    let mut groups = JoinSet::new();
    let mut group_scopes = HashMap::new();
    let mut provider_tracker = JoinSet::new();
    let mut provider_health = JoinSet::new();
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
        health.clone(),
        shutdown_receiver.clone(),
    )));
    workers.spawn(worker_result(lease_worker(
        Arc::clone(&store),
        Arc::clone(&lease),
        shutdown_receiver.clone(),
    )));

    // Resolve every configured scope before starting any native execution.
    let startup: Result<()> = (|| {
        if provider_enabled {
            let factories = crate::provider::session_factories(&config)?;
            let mut prepared = Vec::new();
            for repository in config.github_repositories() {
                let client =
                    github_clients.get(repository).context("repository GitHub client missing")?;
                for kind in [GroupKind::Issue, GroupKind::Pr] {
                    if !config.profiles_for_repository(repository, kind.as_str()).is_empty() {
                        let spec = GroupSpec::new(kind, &config, &store, repository)?;
                        let factory = factories
                            .get(spec.profile_id())
                            .context("Profile session factory missing")?;
                        prepared.push((kind, spec, factory.clone(), client.clone()));
                    }
                }
            }
            let (availability, provider_status) = watch::channel(ProviderAvailability::Starting);
            provider_tracker.spawn(track_provider_availability(
                prepared.len(),
                report_receiver,
                availability,
                group_shutdown.clone(),
            ));
            provider_health.spawn(crate::health::provider_health_worker(
                health.clone(),
                provider_status,
                group_shutdown.clone(),
            ));
            for (kind, spec, factory, client) in prepared {
                let repository = spec.repository.clone();
                let task = groups.spawn(agent_group_worker(
                    Arc::clone(&store),
                    client,
                    config.clone(),
                    spec,
                    factory,
                    reports.clone(),
                    group_shutdown.clone(),
                ));
                group_scopes.insert(task.id(), (repository, kind));
            }
        }

        Ok(())
    })();

    let mut tunnel = JoinSet::new();
    let mut failures = Vec::new();
    let mut group_failures = Vec::new();
    if let Err(error) = startup {
        let diagnostic = format!("Agent Groups could not start: {error:#}");
        tracing::error!("{diagnostic}");
        health.update(|snapshot| {
            snapshot.provider = "unavailable";
            snapshot.last_error = Some(diagnostic);
        });
    }
    {
        if quick_tunnel {
            tunnel.spawn(crate::tunnel::supervise(
                config.clone(),
                github,
                Arc::clone(&state.webhook_secret),
                health.clone(),
                shutdown_receiver,
            ));
        }
        health.update(|current| current.ready = true);
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
                    let diagnostic = message.clone();
                    health.update(|current| {
                        current.ready = false;
                        current.last_error = Some(diagnostic);
                    });
                    failures.push(message);
                    break;
                }
                Some(exit) = join_agent_group(&mut groups, &mut group_scopes) => {
                    let diagnostic = if let Err(error) = exit.result {
                        let message = format!(
                            "Agent Group {} {} stop is unconfirmed: {error:#}",
                            exit.repository, exit.kind.as_str()
                        );
                        group_failures.push(message.clone());
                        message
                    } else if *group_stop.borrow() {
                        continue;
                    } else {
                        format!(
                            "Agent Group {} {} stopped unexpectedly",
                            exit.repository, exit.kind.as_str()
                        )
                    };
                    tracing::error!("{diagnostic}");
                    // The exited scope has no remaining driver. Its failure
                    // stays in the aggregate while healthy siblings report.
                    let _ = reports.send(ProviderReport {
                        repository: exit.repository,
                        group: exit.kind,
                        error: Some(diagnostic),
                    }).await;
                }
                tracker = provider_tracker.join_next(), if !provider_tracker.is_empty() => {
                    let diagnostic = match tracker {
                        Some(Err(error)) => format!("Agent Group provider tracker failed: {error}"),
                        _ => "Agent Group provider tracker stopped unexpectedly".to_owned(),
                    };
                    let _ = group_stop.send(true);
                    provider_health.shutdown().await;
                    tracing::error!("{diagnostic}");
                    health.update(|snapshot| {
                        snapshot.provider = "unavailable";
                        snapshot.last_error = Some(diagnostic);
                    });
                }
                result = tunnel.join_next(), if !tunnel.is_empty() => {
                    let reason = match result {
                        Some(Ok(Err(error))) => format!("Quick Tunnel supervisor failed: {error:#}"),
                        Some(Err(error)) => format!("Quick Tunnel supervisor failed: {error}"),
                        _ => "Quick Tunnel supervisor stopped unexpectedly".into(),
                    };
                    tracing::error!(%reason, "Quick Tunnel can no longer confirm its stop");
                    let diagnostic = reason.clone();
                    health.update(|current| {
                        current.tunnel = "stop_unconfirmed";
                        current.webhook_sync = "blocked";
                        current.tunnel_error = Some(diagnostic);
                    });
                    failures.push(reason);
                }
            }
        }
    }

    health.update(|current| current.ready = false);
    let _ = shutdown_sender.send(true);
    let _ = group_stop.send(true);
    provider_health.shutdown().await;
    provider_tracker.shutdown().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let (workers_stopped, groups_stopped, tunnel_stopped) = tokio::join!(
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
        tokio::time::timeout_at(deadline, async {
            while let Some(exit) = join_agent_group(&mut groups, &mut group_scopes).await {
                if let Err(error) = exit.result {
                    group_failures.push(format!(
                        "Agent Group {} {} did not confirm shutdown: {error:#}",
                        exit.repository,
                        exit.kind.as_str()
                    ));
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
        let reason = "transport workers exceeded the shutdown deadline";
        failures.push(reason.into());
        tracing::error!("{reason}");
        workers.abort_all();
        while workers.join_next().await.is_some() {}
    }
    if groups_stopped.is_err() {
        groups.abort_all();
        while let Some(exit) = join_agent_group(&mut groups, &mut group_scopes).await {
            if let Err(error) = exit.result {
                group_failures.push(format!(
                    "Agent Group {} {} exceeded its shutdown deadline; stop is unconfirmed: {error:#}",
                    exit.repository, exit.kind.as_str()
                ));
            }
        }
    }
    failures.extend(group_failures);
    let outbox_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < outbox_deadline {
        let status = match store.runtime_status() {
            Ok(status) => status,
            Err(error) => {
                failures.push(format!("cannot read transport outbox during shutdown: {error:#}"));
                break;
            }
        };
        if status.pending_writes == 0 && status.uncertain_writes == 0 {
            break;
        }
        let owner = lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        drain_one_write(&store, &github_clients, &config, &owner).await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), ingress).await;
    if let Some(server) = health_server {
        server.abort();
    }
    drop(store);
    if !failures.is_empty() {
        bail!(failures.join("\n"));
    }
    Ok(())
}

struct GroupExit {
    repository: String,
    kind: GroupKind,
    result: Result<()>,
}

async fn join_agent_group(
    groups: &mut JoinSet<Result<()>>,
    scopes: &mut HashMap<Id, (String, GroupKind)>,
) -> Option<GroupExit> {
    let (id, result) = match groups.join_next_with_id().await? {
        Ok((id, result)) => (id, result),
        Err(error) => (error.id(), Err(error.into())),
    };
    let (repository, kind) = scopes.remove(&id).expect("every Agent Group task has a scope");
    Some(GroupExit { repository, kind, result })
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
    state: Health,
    mut shutdown: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let listener = match TcpListener::bind(address).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::warn!(%address, %error, "health endpoint unavailable");
            return None;
        }
    };
    let app = Router::new().route("/healthz", get(health_handler)).with_state(state);
    Some(tokio::spawn(async move {
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

async fn health_handler(State(state): State<Health>) -> impl IntoResponse {
    let mut snapshot = state.snapshot();
    snapshot.ready &= matches!(snapshot.tunnel, "disabled" | "connected");
    let status = if snapshot.ready { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (status, axum::Json(snapshot))
}

pub(crate) async fn builtin_run_managed(
    config: Config,
    request: crate::run::RunRequest,
    store: Arc<StoreActor>,
    stop: watch::Receiver<bool>,
    health: Health,
) -> Result<crate::run::RunResult> {
    builtin::finite_managed(config, request, store, stop, health).await
}
