//! Built-in upstream event bridge and execution lifecycle, backed by the Harness ledger.
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result, bail};
use tokio::{
    sync::watch,
    task::JoinSet,
    time::{Duration, MissedTickBehavior},
};

use crate::{
    builtin::{BuiltinClient, EventFeed},
    collaboration::CollaborationBackend,
    config::Config,
    group::{
        GroupKind, GroupSpec, agent_group_worker_backend,
        availability::{ProviderAvailability, ProviderReport, track_provider_availability},
    },
    health::{Health, HealthSnapshot},
    producer::LEASE_TTL_SECONDS,
    queue::{queue_worker, scheduler::policy_from_config},
    runtime::worker_result,
    store::{RuntimeLease, StoreActor},
};

struct LeaseGuard {
    store: Arc<StoreActor>,
    lease: Arc<Mutex<RuntimeLease>>,
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let lease = self.lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if let Err(error) = self.store.release_runtime_lease(lease) {
            tracing::error!(%error, "cannot release Harness runtime lease");
        }
    }
}

pub(crate) fn initial_health(config: &Config, provider_enabled: bool) -> HealthSnapshot {
    HealthSnapshot {
        ready: !provider_enabled,
        ingress: "builtin".into(),
        repository: config.repository().into(),
        tunnel: "disabled",
        webhook_url: None,
        webhook_sync: "disabled",
        tunnel_verified_at: None,
        tunnel_error: None,
        reconciliation: "builtin",
        provider: if provider_enabled { "starting" } else { "disabled" },
        last_error: None,
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn serve_managed(
    config: Config,
    provider_enabled: bool,
    external_stop: watch::Receiver<bool>,
    health: Health,
) -> Result<()> {
    managed(config, provider_enabled, external_stop, health, None).await.map(|_| ())
}

pub(crate) async fn finite_managed(
    config: Config,
    request: crate::run::RunRequest,
    store: Arc<StoreActor>,
    external_stop: watch::Receiver<bool>,
    health: Health,
) -> Result<crate::run::RunResult> {
    let root = config.finite_run.as_ref().context("finite context missing")?.root;
    let run_id = request.run_id.clone();
    match managed(config, true, external_stop, health, Some((request, store))).await {
        Ok(result) => result.context("finite runtime did not return a result"),
        Err(error) => Ok(crate::run::RunResult {
            run_id,
            root_issue: root,
            state: crate::run::RunState::Failed,
            delivery_commit: None,
            reason: Some(format!("runtime startup failed before execution: {error:#}")),
            stop_confirmed: true,
        }),
    }
}

// The shutdown barrier shares the worker resources established during startup.
#[allow(clippy::too_many_lines)]
async fn managed(
    config: Config,
    provider_enabled: bool,
    mut external_stop: watch::Receiver<bool>,
    health: Health,
    finite: Option<(crate::run::RunRequest, Arc<StoreActor>)>,
) -> Result<Option<crate::run::RunResult>> {
    if *external_stop.borrow() {
        return Ok(finite.as_ref().map(|(request, _)| crate::run::RunResult {
            run_id: request.run_id.clone(),
            root_issue: config.finite_run.as_ref().expect("finite context").root,
            state: crate::run::RunState::Stopped,
            delivery_commit: None,
            reason: Some("stopped before execution".into()),
            stop_confirmed: true,
        }));
    }
    let initial = initial_health(&config, provider_enabled);
    health.update(|current| *current = initial);
    let store = if let Some((_, store)) = &finite {
        Arc::clone(store)
    } else {
        Arc::new(StoreActor::start(
            config.runtime.database().to_path_buf(),
            config.runtime.backups().to_path_buf(),
        )?)
    };
    if !store.plan()?.pending.is_empty() {
        if config.runtime.auto_migrate {
            store.apply()?;
        } else {
            bail!("database needs migration; run `braid migrate apply --config <PATH>`");
        }
    }
    let lease = Arc::new(Mutex::new(store.acquire_runtime_lease(
        "runtime".into(),
        uuid::Uuid::now_v7().to_string(),
        LEASE_TTL_SECONDS,
    )?));
    let _lease_guard = LeaseGuard { store: Arc::clone(&store), lease: Arc::clone(&lease) };
    let client = BuiltinClient::connect(&config, Arc::clone(&store)).await?;
    let write_owner = lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    let (shutdown, receiver) = watch::channel(false);
    let (group_shutdown, group_receiver) = watch::channel(false);
    let health_server =
        crate::runtime::spawn_health(config.server.health, health.clone(), receiver.clone()).await;
    let mut workers = JoinSet::new();
    let mut group_workers = JoinSet::new();
    let mut group_scopes = HashMap::new();
    let mut bootstrap = JoinSet::new();
    let mut provider_health = JoinSet::new();
    let mut provider_reports = JoinSet::new();
    workers.spawn(worker_result(queue_worker(Arc::clone(&store), receiver.clone())));
    workers.spawn(worker_result(crate::producer::lease_worker(
        Arc::clone(&store),
        Arc::clone(&lease),
        receiver.clone(),
    )));
    workers.spawn(write_worker(
        write_owner.clone(),
        Arc::clone(&store),
        Arc::clone(&client),
        receiver.clone(),
    ));
    workers.spawn(object_worker(
        Arc::clone(&store),
        Arc::clone(&client),
        config.clone(),
        health.clone(),
        receiver.clone(),
    ));
    let (availability, provider_status) = watch::channel(ProviderAvailability::Starting);
    let (reports, report_receiver) = tokio::sync::mpsc::channel(8);
    let mut report_receiver = Some(report_receiver);
    if provider_enabled {
        bootstrap.spawn(prepare_agent_groups(
            config.clone(),
            Arc::clone(&store),
            write_owner.clone(),
            availability.clone(),
        ));
        provider_health.spawn(crate::health::provider_health_worker(
            health.clone(),
            provider_status.clone(),
            receiver.clone(),
        ));
    }
    let mut execution_client = None;
    tracing::info!(repository = %config.repository(), "Braid built-in Harness runtime is ready");
    let mut outcome = None;
    let mut execution_failures = Vec::new();
    let result = loop {
        tokio::select! {
            biased;
            _ = external_stop.changed() => {
                if let Some((request,_))=&finite {
                    outcome=Some(crate::run::RunResult {run_id:request.run_id.clone(),root_issue:config.finite_run.as_ref().expect("finite context").root,state:crate::run::RunState::Stopped,delivery_commit:None,reason:Some("external stop requested".into()),stop_confirmed:false});
                }
                break Ok(());
            },
            next = finite_outcome(&config,&client,&store,&provider_status), if finite.is_some() => {
                break match next {Ok(result)=> {outcome=Some(result);Ok(())}, Err(error)=>Err(error)};
            },
            prepared = bootstrap.join_next(), if !bootstrap.is_empty() => {
                let Some(Ok((bound_client, groups))) = prepared else {
                    break Err(anyhow::anyhow!("built-in execution bootstrap task failed"));
                };
                let backend = Arc::new(CollaborationBackend::Builtin(Arc::clone(&bound_client)));
                execution_client = Some(bound_client);
                provider_reports.spawn(track_provider_availability(
                    groups.len(), report_receiver.take().expect("execution bootstrap completes once"),
                    availability.clone(), group_receiver.clone(),
                ));
                for (kind, spec, factory) in groups {
                    let repository = spec.repository.clone();
                    let task = group_workers.spawn(agent_group_worker_backend(
                        Arc::clone(&store), Arc::clone(&backend), config.clone(), spec, factory,
                        reports.clone(), group_receiver.clone(),
                    ));
                    group_scopes.insert(task.id(), (repository, kind));
                }
            },
            Some(exit) = super::join_agent_group(&mut group_workers, &mut group_scopes) => {
                let scope = format!("Agent Group {} {}", exit.repository, exit.kind.as_str());
                let error = match exit.result {
                    Err(error) => {
                        let diagnostic = format!("{scope} stop is unconfirmed: {error:#}");
                        execution_failures.push(diagnostic.clone());
                        diagnostic
                    }
                    Ok(()) if *group_receiver.borrow() => continue,
                    Ok(()) => format!("{scope} stopped unexpectedly"),
                };
                tracing::error!(%error, "execution scope stopped while collaboration intake continues");
                let _ = reports.send(ProviderReport { repository: exit.repository, group: exit.kind, error: Some(error) }).await;
            },
            tracker = provider_reports.join_next(), if !provider_reports.is_empty() => {
                let error = match tracker {
                    Some(Err(error)) => format!("built-in Provider availability tracker failed: {error}"),
                    _ => "built-in Provider availability tracker stopped unexpectedly".into(),
                };
                tracing::error!(%error, "execution stopped while collaboration intake continues");
                let _ = group_shutdown.send(true);
                availability.send_replace(ProviderAvailability::Unavailable { since: tokio::time::Instant::now(), error });
            },
            worker = workers.join_next() => break match worker {
                Some(Ok(Err(error))) => Err(error.context("built-in collaboration worker failed")),
                Some(Err(error)) => Err(anyhow::Error::new(error).context("built-in Harness worker failed")),
                Some(Ok(Ok(()))) => Err(anyhow::anyhow!("built-in Harness worker stopped unexpectedly")),
                None => Err(anyhow::anyhow!("all built-in Harness workers stopped unexpectedly")),
            },
        }
    };
    provider_health.shutdown().await;
    let runtime_error = result.err().map(|error| format!("{error:#}"));
    let diagnostic = runtime_error.clone();
    health.update(|snapshot| {
        snapshot.ready = false;
        if diagnostic.is_some() {
            snapshot.last_error = diagnostic;
        }
    });
    let _ = shutdown.send(true);
    let _ = group_shutdown.send(true);
    bootstrap.shutdown().await;
    provider_reports.shutdown().await;
    let mut failures = Vec::new();
    let deadline = finite.is_some().then(|| tokio::time::Instant::now() + Duration::from_secs(15));
    tokio::join!(
        settle_workers(&mut workers, &mut failures, deadline, "collaboration workers"),
        async {
            settle_workers(&mut group_workers, &mut execution_failures, deadline, "Agent Groups")
                .await;
            if provider_enabled {
                let fenced = async {
                    let execution = match execution_client {
                        Some(client) => client,
                        None => client.with_runtime(write_owner.clone())?,
                    };
                    execution.fence_all_executions().await
                }
                .await;
                if let Err(error) = fenced {
                    execution_failures
                        .push(format!("built-in execution fence remains unconfirmed: {error:#}"));
                }
            }
        },
    );
    let stop_confirmed = execution_failures.is_empty();
    failures.extend(execution_failures);
    if finite.is_some() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let status = match store.publication_progress() {
                Ok(status) => status,
                Err(error) => {
                    failures.push(format!("shutdown ledger status failed: {error:#}"));
                    break;
                }
            };
            if status.pending == 0 {
                break;
            }
            let owner = lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
            if let Err(error) = apply_next_write(&owner, &store, &client).await {
                if let Some(result) = &mut outcome {
                    result.state = crate::run::RunState::Blocked;
                    result.reason = Some(format!("shutdown outbox failed: {error:#}"));
                }
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let status = store.publication_progress();
        if let Ok(status) = status {
            if (status.pending > 0 || status.uncertain > 0)
                && let Some(result) = &mut outcome
            {
                result.state = crate::run::RunState::Blocked;
                result.reason = Some(format!(
                    "shutdown left {} pending and {} uncertain platform writes",
                    status.pending, status.uncertain
                ));
            }
        } else if let Err(error) = status {
            failures.push(format!("shutdown ledger status failed: {error:#}"));
        }
    }
    if let Some(server) = health_server {
        server.abort();
    }
    health.update(|snapshot| snapshot.ready = false);
    let generation = crate::telemetry::evidence_health().runtime_generation;
    tracing::info!(
        name="braid.runtime.stop",
        braid.runtime.generation=generation.as_deref().unwrap_or(""),
        braid.runtime.stop_confirmed=stop_confirmed,
        braid.runtime.ready=false,
        braid.runtime.error=%failures.join("\n"),
        "built-in Harness runtime stopped"
    );
    if let Some((request, _)) = &finite {
        let mut result = outcome.unwrap_or(crate::run::RunResult {
            run_id: request.run_id.clone(),
            root_issue: config.finite_run.as_ref().expect("finite context").root,
            state: crate::run::RunState::Failed,
            delivery_commit: None,
            reason: runtime_error.clone(),
            stop_confirmed: false,
        });
        result.stop_confirmed = stop_confirmed;
        if !failures.is_empty() {
            result.state = crate::run::RunState::Blocked;
            result.reason =
                Some([result.reason.unwrap_or_default(), failures.join("\n")].join("\n"));
        }
        return Ok(Some(result));
    }
    if let Some(error) = runtime_error {
        failures.insert(0, error);
    }
    if failures.is_empty() {
        Ok(None)
    } else {
        let error = failures.join("\n");
        let diagnostic = error.clone();
        health.update(|snapshot| snapshot.last_error = Some(diagnostic));
        bail!(error)
    }
}

type PreparedGroups = Vec<(GroupKind, GroupSpec, Arc<dyn crate::agent_session::SessionFactory>)>;

async fn prepare_agent_groups(
    config: Config,
    store: Arc<StoreActor>,
    lease: RuntimeLease,
    availability: watch::Sender<ProviderAvailability>,
) -> (Arc<BuiltinClient>, PreparedGroups) {
    let mut unavailable_since = None;
    loop {
        let prepared: Result<_> = async {
            let client = BuiltinClient::connect(&config, store.clone())
                .await?
                .with_runtime(lease.clone())?;
            client.fence_all_executions().await.context("cannot fence old built-in executions")?;
            let factories = crate::provider::session_factories(&config)?;
            let mut groups = Vec::new();
            for kind in [GroupKind::Issue, GroupKind::Pr] {
                let spec = GroupSpec::new(kind, &config, &store, config.repository())?;
                let factory = factories
                    .get(spec.profile_id())
                    .context("Profile session factory missing")?
                    .clone();
                groups.push((kind, spec, factory));
            }
            Ok((client, groups))
        }
        .await;
        match prepared {
            Ok(ready) => {
                availability.send_replace(ProviderAvailability::Starting);
                return ready;
            }
            Err(error) => {
                let since = *unavailable_since.get_or_insert_with(tokio::time::Instant::now);
                tracing::warn!(%error, "built-in execution admission is unavailable");
                availability.send_replace(ProviderAvailability::Unavailable {
                    since,
                    error: format!("{error:#}"),
                });
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
}

async fn settle_workers(
    workers: &mut JoinSet<Result<()>>,
    failures: &mut Vec<String>,
    deadline: Option<tokio::time::Instant>,
    responsibility: &str,
) {
    let cleanup = async {
        while let Some(worker) = workers.join_next().await {
            match worker {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("{error:#}")),
                Err(error) => {
                    failures.push(format!("{responsibility} failed during shutdown: {error}"));
                }
            }
        }
    };
    if let Some(deadline) = deadline {
        if tokio::time::timeout_at(deadline, cleanup).await.is_err() {
            failures.push(format!("{responsibility} exceeded the shutdown deadline"));
            workers.shutdown().await;
        }
    } else {
        cleanup.await;
    }
}

async fn object_worker(
    store: Arc<StoreActor>,
    client: Arc<BuiltinClient>,
    config: Config,
    health: Health,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tick.tick() => {
                match advance_objects(&store, &client, &config).await {
                    Ok(()) => health.update(|snapshot| snapshot.reconciliation = "builtin"),
                    Err(error) if error.is::<reqwest::Error>()
                        || error.downcast_ref::<crate::builtin::PlatformHttpError>()
                            .is_some_and(|http| http.status.is_server_error()) =>
                    {
                        tracing::warn!(%error, "built-in event feed unavailable; keeping its durable cursor");
                        health.update(|snapshot| snapshot.reconciliation = "builtin-degraded");
                        tick.reset_after(Duration::from_secs(2));
                    }
                    Err(error) => return Err(error.context("cannot advance built-in collaboration")),
                }
            }
        }
    }
}

fn client_cursor(store: &StoreActor) -> Result<u64> {
    let crate::store::BuiltinReply::Binding(binding) =
        store.builtin(crate::store::BuiltinCommand::State)?
    else {
        anyhow::bail!("built-in upstream is not bound")
    };
    Ok(binding.cursor)
}

async fn finite_boundary(
    client: &BuiltinClient,
    run: &crate::run::RunContext,
) -> Result<crate::platform::objects::ScopeBoundary> {
    let boundary = client.scope_boundary(run.root).await?;
    anyhow::ensure!(
        boundary.matches_registration(
            run.root,
            &run.run_id,
            &client.binding.client_id,
            &run.delivery_ref,
        ),
        "platform finite-run identity or delivery policy changed"
    );
    if !boundary.root_open {
        run.admission.close();
    }
    Ok(boundary)
}

#[allow(clippy::too_many_lines)]
async fn advance_objects(
    store: &StoreActor,
    client: &BuiltinClient,
    config: &Config,
) -> Result<()> {
    let feed: EventFeed = client
        .get(&format!("api/platform/events?after={}&limit=100", client_cursor(store)?))
        .await?;
    let scope = if let Some(run) = &config.finite_run
        && !feed.events.is_empty()
    {
        Some(finite_boundary(client, run).await?.member_numbers)
    } else {
        None
    };
    let mut cursor = client_cursor(store)?;
    anyhow::ensure!(
        feed.last_sequence >= cursor,
        "platform event history regressed behind the Harness cursor"
    );
    for pending in feed.events {
        anyhow::ensure!(
            pending.sequence > cursor && pending.sequence <= feed.last_sequence,
            "platform events are not in immutable increasing order"
        );
        let event = pending.event.into_ingress()?;
        anyhow::ensure!(
            event.repository == config.repository()
                && event.repository_node_id == format!("local:{}", config.repository()),
            "platform event belongs to a different collaboration authority"
        );
        if scope.as_ref().is_some_and(|scope| {
            event.work_item_number.is_none_or(|number| !scope.contains(&number))
        }) {
            store.builtin(crate::store::BuiltinCommand::Advance(pending.sequence))?;
            cursor = pending.sequence;
            continue;
        }
        let delivery_guid = event.delivery_guid.clone();
        let number = event.work_item_number;
        let item_kind = event.work_item_kind;
        let queue_kind = event.kind.as_str();
        let generation = crate::telemetry::evidence_health().runtime_generation;
        let ingest =
            store.ingest_builtin_event(event, policy_from_config(config), pending.sequence);
        tracing::info!(
            name="braid.local.event_ingest",
            braid.runtime.generation=generation.as_deref().unwrap_or(""),
            braid.local.event_id=pending.sequence,
            braid.delivery.guid=%delivery_guid,
            braid.work_item.number=number,
            braid.work_item.kind=item_kind.unwrap_or(""),
            braid.queue.kind=queue_kind,
            braid.event.accepted=ingest.is_ok(),
            braid.event.duplicate=ingest.as_ref().ok().map(|result|result.duplicate),
            braid.event.id=ingest.as_ref().ok().and_then(|result|result.event_id.as_deref()).unwrap_or(""),
            braid.event.lifecycle=ingest.as_ref().ok().and_then(|result|result.event_lifecycle.as_deref()).unwrap_or(""),
            braid.queue.batch_id=ingest.as_ref().ok().and_then(|result|result.batch_id.as_deref()).unwrap_or(""),
            braid.queue.lifecycle=ingest.as_ref().ok().and_then(|result|result.batch_lifecycle.as_deref()).unwrap_or(""),
            braid.event.error=ingest.as_ref().err().map(ToString::to_string).as_deref().unwrap_or(""),
            "built-in upstream event ingested"
        );
        // Event, queued input and cursor are committed together.
        tracing::info!(
            name="braid.builtin.cursor_advance",
            braid.runtime.generation=generation.as_deref().unwrap_or(""),
            braid.local.event_id=pending.sequence,
            braid.delivery.guid=%delivery_guid,
            braid.work_item.number=number,
            braid.work_item.kind=item_kind.unwrap_or(""),
            braid.queue.kind=queue_kind,
            braid.event.cursor_saved=ingest.is_ok(),
            braid.event.error=ingest.as_ref().err().map(|error|format!("{error:#}")).as_deref().unwrap_or(""),
            "Harness upstream cursor advanced with event intake"
        );
        ingest?;
        cursor = pending.sequence;
    }
    anyhow::ensure!(
        feed.next_after == cursor,
        "platform nextAfter does not match delivered event sequence"
    );
    Ok(())
}

async fn write_worker(
    owner: RuntimeLease,
    store: Arc<StoreActor>,
    client: Arc<BuiltinClient>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut recovered = false;
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tick.tick() => {
                if !recovered {
                    match store.recover_writes() {
                        Ok(_) => recovered = true,
                        Err(error) => {
                            tracing::error!(%error, "cannot recover built-in outbox");
                            continue;
                        }
                    }
                }
                if let Err(error) = apply_next_write(&owner, &store, &client).await {
                    tracing::error!(%error, "cannot settle built-in outbox");
                }
            }
        }
    }
}

async fn apply_next_write(
    owner: &RuntimeLease,
    store: &StoreActor,
    client: &BuiltinClient,
) -> Result<()> {
    if let Some(write) = store.claim_github_write(owner.clone())? {
        match client.apply_write(&write).await {
            Ok((database_id, node_id)) => {
                tracing::info!(
                    name="braid.local.outbox_result",
                    braid.runtime.generation=crate::telemetry::evidence_health().runtime_generation.as_deref().unwrap_or(""),
                    braid.outbox.intent_id=%write.intent_id,
                    braid.outbox.operation=%write.operation,
                    braid.outbox.lifecycle="applied",
                    "built-in upstream write applied"
                );
                store.finish_github_write(write.clone(), "applied", database_id, node_id, None)?;
            }
            Err(error) => {
                let lifecycle = if error
                    .downcast_ref::<crate::builtin::PlatformHttpError>()
                    .is_some_and(|error| error.status.is_client_error())
                {
                    "rejected"
                } else {
                    "uncertain"
                };
                tracing::error!(
                    name="braid.local.outbox_result",
                    braid.runtime.generation=crate::telemetry::evidence_health().runtime_generation.as_deref().unwrap_or(""),
                    braid.outbox.intent_id=%write.intent_id,
                    braid.outbox.operation=%write.operation,
                    braid.outbox.lifecycle=lifecycle,
                    braid.outbox.error=%format!("{error:#}"),
                    "built-in upstream write failed"
                );
                store.finish_github_write(
                    write.clone(),
                    lifecycle,
                    None,
                    None,
                    Some(format!("{error:#}")),
                )?;
            }
        }
    }
    Ok(())
}

// Observe one bounded run's terminal decision in a single polling loop.
#[allow(clippy::too_many_lines)]
async fn finite_outcome(
    config: &Config,
    client: &BuiltinClient,
    store: &StoreActor,
    provider_status: &watch::Receiver<ProviderAvailability>,
) -> Result<crate::run::RunResult> {
    let run = config.finite_run.as_ref().context("finite context missing")?;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        if finite_boundary(client, run).await?.root_open {
            let progress = store.execution_progress()?;
            if let ProviderAvailability::Unavailable { since, error } = &*provider_status.borrow()
                && since.elapsed() >= Duration::from_secs(6)
            {
                return Ok(crate::run::RunResult {
                    run_id: run.run_id.clone(),
                    root_issue: run.root,
                    state: crate::run::RunState::Blocked,
                    delivery_commit: None,
                    reason: Some(format!(
                        "provider remained unavailable through recovery attempts: {error}"
                    )),
                    stop_confirmed: false,
                });
            }
            if run.admission.is_closed() {
                return Ok(crate::run::RunResult {run_id:run.run_id.clone(),root_issue:run.root,state:crate::run::RunState::Blocked,delivery_commit:None,reason:Some("execution admission fenced after an unconfirmed physical stop; inspect retained session stop evidence".into()),stop_confirmed:false});
            }
            if let Some(group) = progress.blocked_group {
                return Ok(crate::run::RunResult {run_id:run.run_id.clone(),root_issue:run.root,state:crate::run::RunState::Blocked,delivery_commit:None,reason:Some(format!("{} #{} execution is blocked: {}",group.kind,group.number,group.error.as_deref().unwrap_or("provider or assignment could not recover; inspect retained execution error"))),stop_confirmed:false});
            }
            if let Some(reset) = progress.blocked_reset {
                return Ok(crate::run::RunResult {
                    run_id: run.run_id.clone(),
                    root_issue: run.root,
                    state: crate::run::RunState::Blocked,
                    delivery_commit: None,
                    reason: Some(format!(
                        "context reset {reset} is blocked; inspect retained reset error"
                    )),
                    stop_confirmed: false,
                });
            }
            continue;
        }
        let scope: crate::platform::objects::ScopeSnapshot = client
            .get(&format!("api/platform/scope/{}", run.root))
            .await
            .context("cannot read finite-run terminal delivery report")?;
        anyhow::ensure!(
            scope.root.number == run.root
                && scope.run_id.as_deref() == Some(&run.run_id)
                && scope.client_id.as_deref() == Some(&client.binding.client_id)
                && scope.delivery_ref.as_deref() == Some(&run.delivery_ref),
            "platform finite-run identity or delivery policy changed"
        );
        let mut result = crate::run::RunResult {
            run_id: run.run_id.clone(),
            root_issue: run.root,
            state: crate::run::RunState::Blocked,
            delivery_commit: None,
            reason: None,
            stop_confirmed: false,
        };
        if scope.root.state == "open" {
            result.reason = Some("root reopened after execution admission closed".into());
            return Ok(result);
        }
        let unfinished: Vec<_> = scope
            .issues
            .iter()
            .chain(&scope.pull_requests)
            .filter(|item| item.state == "open")
            .map(|item| format!("{} #{}", item.kind, item.number))
            .collect();
        if !unfinished.is_empty() {
            result.reason =
                Some(format!("root closed before obligations settled: {}", unfinished.join(", ")));
            return Ok(result);
        }
        if let Some(merge) = scope.pending_merges.iter().find(|merge| {
            matches!(merge.lifecycle.as_str(), "conflict" | "failed" | "rejected" | "uncertain")
        }) {
            result.reason = Some(format!(
                "PR #{} merge {}: {}",
                merge.number,
                merge.lifecycle,
                merge.error.as_deref().unwrap_or("no detailed error recorded")
            ));
            return Ok(result);
        }
        if !scope.prepared_commands.is_empty() || !scope.pending_merges.is_empty() {
            continue;
        }
        let publication = store.publication_progress()?;
        if publication.uncertain > 0 {
            result.reason = Some(
                "root closed with uncertain platform writes; inspect retained outbox receipts"
                    .into(),
            );
            return Ok(result);
        }
        if store.execution_progress()?.unresolved_reset {
            result.reason=Some("root closed with an unresolved context reset; no replacement execution is admitted".into());
            return Ok(result);
        }
        if publication.pending > 0 {
            continue;
        }
        let Some(commit) = scope.delivery_commit else {
            result.reason =
                Some(format!("root closed without a commit at delivery ref {}", run.delivery_ref));
            return Ok(result);
        };
        let delivered = scope.deliveries.iter().any(|delivery| {
            delivery.merge_commit.as_deref() == Some(&commit)
                || delivery.published && delivery.head_commit.as_deref() == Some(&commit)
        });
        if !delivered {
            result.reason = Some(format!(
                "root closed without a published or merged scoped candidate matching delivery ref {} ({commit})",
                run.delivery_ref
            ));
            return Ok(result);
        }
        result.state = crate::run::RunState::Completed;
        result.delivery_commit = Some(commit);
        return Ok(result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::get};

    #[tokio::test]
    async fn managed_ingests_while_old_execution_fence_is_unavailable() {
        let fixture = crate::writer::tests::Fixture::new(false);
        let store = Arc::new(
            StoreActor::start(
                fixture.config.runtime.database().into(),
                fixture.config.runtime.backups().into(),
            )
            .unwrap(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route(
                "/api/platform/repository",
                get(|| async {
                    Json(serde_json::json!({
                        "repository": "test-space", "spaceId": "space", "executionFences": true,
                        "nodeId": "local:test-space", "defaultBranch": "main"
                    }))
                }),
            )
            .route(
                "/api/platform/events",
                get(|| async {
                    Json(serde_json::json!({"events": [{
                    "sequence": 1, "event": {
                        "delivery_guid": "local:1", "event_name": "local",
                        "repository": "test-space", "repository_node_id": "local:test-space",
                        "work_item_node_id": "local:test-space:issue:1", "work_item_kind": "issue",
                        "work_item_number": 1, "work_item_state": "closed",
                        "kind": "lifecycle", "detail": "closed",
                        "cross_surface_invalidation": false, "origin": "human",
                        "reference": "Issue closed", "mention_candidate": false,
                        "known": true, "raw_payload": []
                    }
                }], "nextAfter": 1, "lastSequence": 1}))
                }),
            )
            .route(
                "/api/platform/executions/fence",
                axum::routing::post(|| async { axum::http::StatusCode::SERVICE_UNAVAILABLE }),
            );
        let http = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = fixture.config.clone();
        config.github = None;
        config.builtin = Some(crate::config::BuiltinConfig {
            repository: "test-space".into(),
            endpoint: format!("http://{address}").parse().unwrap(),
        });
        config.server.health = "127.0.0.1:0".parse().unwrap();
        fixture.db().execute_batch(
        "INSERT INTO repositories VALUES('local:test-space','test-space',1,'now');
         INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
            VALUES('local:test-space:issue:1','local:test-space','issue',1,'open','now');
         INSERT INTO profiles VALUES('old',1,printf('%064d',0),'pi','[]');
         INSERT INTO assignments VALUES('assignment','local:test-space:issue:1',1,'active','now',NULL);
         INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle)
            VALUES('agent','assignment','old',1,'issue','idle');
         INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,
             instruction_revision,lifecycle,started_at,builtin_execution_id,builtin_execution_owner)
            VALUES('session','agent','pi','native','memory','guide','idle','now',
                   '018f8191-e205-7e46-881b-b11c83b56cfd','previous-runtime');",
    ).unwrap();

        let health = Health::new(initial_health(&config, true));
        let (halt, stop) = watch::channel(false);
        let runtime = tokio::spawn(serve_managed(config, true, stop, health.clone()));
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let watching_fence = health
                    .snapshot()
                    .last_error
                    .as_deref()
                    .is_some_and(|error| error.contains("execution fence"));
                if watching_fence && client_cursor(&store).is_ok_and(|cursor| cursor == 1) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("feed reception must survive a failed execution restore");
        let events =
            store.work_item_lifecycle_candidates("test-space".into(), "issue".into(), 1).unwrap();
        assert_eq!(events[0].action, "closed");
        let turns: u64 = fixture
            .db()
            .query_row("SELECT COUNT(*) FROM turns", [], |row| row.get::<_, i64>(0))
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(turns, 0);
        halt.send(true).unwrap();
        let error = runtime.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("execution fence remains unconfirmed"), "{error:#}");
        http.abort();
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn finite_input_and_open_outcome_survive_unreadable_delivery_ref() {
        use crate::platform::{
            PlatformConfig,
            objects::{CommandRequest, ControlReceipt, ObjectCommand, PlatformObjects},
        };
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::sync::Notify;

        const DELIVERY_REF: &str = "refs/heads/delivery";
        let fixture = crate::writer::tests::Fixture::new(false);
        let runtime = fixture.config.runtime.database().parent().unwrap();
        let source = runtime.join("source");
        let git = git2::Repository::init_opts(
            &source,
            git2::RepositoryInitOptions::new().initial_head("main"),
        )
        .unwrap();
        let signature = git2::Signature::now("Braid Test", "test@example.invalid").unwrap();
        let tree = git.find_tree(git.treebuilder(None).unwrap().write().unwrap()).unwrap();
        let base = git.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[]).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let declaration = PlatformConfig {
            schema_version: 1,
            repository: "test-space".into(),
            source: source.canonicalize().unwrap(),
            default_branch: "main".into(),
            database: runtime.join("platform.sqlite3"),
            backups: runtime.join("platform-backups"),
            listen: address.to_string(),
        };
        let two_open_polls = Arc::new(Notify::new());
        let routes =
            crate::platform::router(&declaration).unwrap().layer(axum::middleware::from_fn({
                let two_open_polls = Arc::clone(&two_open_polls);
                let boundary_reads = Arc::new(AtomicUsize::new(0));
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let two_open_polls = Arc::clone(&two_open_polls);
                    let boundary_reads = Arc::clone(&boundary_reads);
                    async move {
                        let boundary = request.uri().path().ends_with("/boundary");
                        let response = next.run(request).await;
                        // Event intake reads once, followed by two finite outcome polls.
                        if boundary && boundary_reads.fetch_add(1, Ordering::Relaxed) == 2 {
                            two_open_polls.notify_one();
                        }
                        response
                    }
                }
            }));
        let http = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });

        let mut platform =
            PlatformObjects::open(&declaration.database, &declaration.repository).unwrap();
        let root = platform.create_child_issue("Root", "Work item", "human", None).unwrap();
        let child =
            platform.create_child_issue("Child", "Work item", "human", Some(root.number)).unwrap();
        let mut config = fixture.config.clone();
        config.github = None;
        config.builtin = Some(crate::config::BuiltinConfig {
            repository: declaration.repository.clone(),
            endpoint: format!("http://{address}").parse().unwrap(),
        });
        let admission = Arc::new(tokio::sync::Semaphore::new(1));
        config.finite_run = Some(crate::run::RunContext {
            run_id: "test-run".into(),
            root: root.number,
            delivery_ref: DELIVERY_REF.into(),
            admission: Arc::clone(&admission),
        });
        let store = Arc::new(
            StoreActor::start(config.runtime.database().into(), config.runtime.backups().into())
                .unwrap(),
        );
        let client = BuiltinClient::connect(&config, Arc::clone(&store)).await.unwrap();
        let git_directory = source.join(".git");
        let offline_directory = source.join(".git.offline");
        std::fs::rename(&git_directory, &offline_directory).unwrap();
        let receipt: ControlReceipt = client
            .post(
                "api/control/commands",
                &CommandRequest {
                    request_id: "test-run-boundary".into(),
                    expected_revision: Some(platform.item(root.number).unwrap().revision),
                    command: ObjectCommand::ConfigureRunRoot {
                        number: root.number,
                        client_id: client.binding.client_id.clone(),
                        run_id: "test-run".into(),
                        delivery_ref: DELIVERY_REF.into(),
                    },
                    actor: "human".into(),
                    writer: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(receipt.state, "applied");
        std::fs::rename(&offline_directory, &git_directory).unwrap();
        assert!(git.find_reference(DELIVERY_REF).is_err());
        let candidate = git
            .commit(
                None,
                &signature,
                &signature,
                "candidate",
                &tree,
                &[&git.find_commit(base).unwrap()],
            )
            .unwrap();
        let pr = platform
            .create_pr(
                "Candidate",
                "Change",
                "human",
                &source,
                &candidate.to_string(),
                DELIVERY_REF,
                Some(child.number),
            )
            .unwrap();
        platform.link(root.number, pr.number, "human").unwrap();
        platform.publish(pr.number, &declaration.source, &candidate.to_string(), "human").unwrap();
        assert_eq!(git.refname_to_id(DELIVERY_REF).unwrap(), base);
        let merged = platform.merge(pr.number, &declaration.source, "human").unwrap().merge_commit;
        assert_eq!(git.refname_to_id(DELIVERY_REF).unwrap().to_string(), merged);
        let db = rusqlite::Connection::open(&declaration.database).unwrap();
        db.execute_batch("ALTER TABLE local_merges RENAME TO offline_merges").unwrap();
        assert_eq!(client.candidate(pr.number).await.unwrap(), candidate.to_string());
        let root_view: serde_json::Value =
            client.get(&format!("api/control/items/{}", root.number)).await.unwrap();
        assert_eq!(root_view["item"]["number"], root.number);
        let context = client.context(pr.number).await.unwrap();
        assert!(matches!(context, crate::context::CanonicalContext::PullRequest(pr) if pr.merged));
        assert!(platform.set_state(pr.number, true, "human").is_err());
        db.execute_batch("ALTER TABLE offline_merges RENAME TO local_merges").unwrap();
        platform.set_state(child.number, false, "human").unwrap();
        let outside = platform.create_child_issue("Outside", "Other run", "human", None).unwrap();
        platform.set_state(outside.number, false, "human").unwrap();

        let healthy: crate::platform::objects::ScopeSnapshot =
            client.get(&format!("api/platform/scope/{}", root.number)).await.unwrap();
        assert_eq!(healthy.delivery_commit.as_deref(), Some(merged.as_str()));
        assert_eq!(healthy.pull_requests.len(), 1);
        let blob = git.blob(b"delivery ref cannot peel to a commit").unwrap();
        let delivery_path = git.path().join(DELIVERY_REF);
        std::fs::write(&delivery_path, format!("{blob}\n")).unwrap();
        assert!(git.find_reference(DELIVERY_REF).unwrap().peel_to_commit().is_err());

        advance_objects(&store, &client, &config).await.unwrap();
        assert_eq!(
            client_cursor(&store).unwrap(),
            platform.events(0, 100).unwrap().last().unwrap().event_id
        );
        for (kind, member) in [("issue", child.number), ("pr", pr.number)] {
            let events =
                store.work_item_lifecycle_candidates("test-space".into(), kind.into(), 10).unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].number, member);
            assert_eq!(events[0].action, if kind == "pr" { "merged" } else { "closed" });
        }

        let (_availability, provider_status) = watch::channel(ProviderAvailability::Starting);
        let outcome = finite_outcome(&config, &client, &store, &provider_status);
        tokio::pin!(outcome);
        tokio::time::timeout(Duration::from_secs(5), async {
            tokio::select! {
                result = &mut outcome => panic!("open root stopped polling: {result:?}"),
                () = two_open_polls.notified() => {},
            }
        })
        .await
        .unwrap();
        assert!(!admission.is_closed());
        platform.set_state(root.number, false, "human").unwrap();
        let error =
            tokio::time::timeout(Duration::from_secs(5), &mut outcome).await.unwrap().unwrap_err();
        assert!(admission.is_closed());
        assert_eq!(error.to_string(), "cannot read finite-run terminal delivery report");
        assert_eq!(
            error.downcast_ref::<crate::builtin::PlatformHttpError>().unwrap().status,
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        );
        std::fs::write(&delivery_path, format!("{merged}\n")).unwrap();
        let result = finite_outcome(&config, &client, &store, &provider_status).await.unwrap();
        assert_eq!(result.state, crate::run::RunState::Completed);
        assert_eq!(result.delivery_commit, Some(merged));
        http.abort();
    }
}
