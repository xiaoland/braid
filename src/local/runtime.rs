//! Long-lived local collaboration runtime; object mutations feed the shared scheduler.
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use tokio::{
    sync::{RwLock, watch},
    task::JoinSet,
    time::{Duration, MissedTickBehavior},
};

use crate::{
    collaboration::CollaborationBackend,
    config::Config,
    group::{GroupKind, GroupSpec, agent_group_worker_backend},
    health::HealthSnapshot,
    local::objects::LocalObjects,
    producer::LEASE_TTL_SECONDS,
    queue::{queue_worker, scheduler::policy_from_config},
    runtime::worker_result,
    store::{RuntimeLease, StoreActor},
    telemetry::TelemetryGuard,
};

struct LeaseGuard {
    store: Arc<StoreActor>,
    lease: Arc<Mutex<RuntimeLease>>,
}
impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let lease = self.lease.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if let Err(error) = self.store.release_runtime_lease(lease) {
            tracing::error!(%error, "cannot release local runtime lease");
        }
    }
}

pub(crate) async fn serve(config: Config, provider_enabled: bool) -> Result<()> {
    let telemetry = TelemetryGuard::install(&config.telemetry, config.instance_key())?;
    let generation = uuid::Uuid::now_v7().to_string();
    crate::telemetry::set_runtime_generation(Some(generation.clone()));
    let health = Arc::new(RwLock::new(initial_health(&config, provider_enabled)));
    let (stop, shutdown) = watch::channel(false);
    let mut runtime = Box::pin(serve_managed(config, provider_enabled, shutdown, health));
    let result = tokio::select! {
        result = &mut runtime => result,
        signal = crate::runtime::wait_for_shutdown_signal() => {
            let _ = stop.send(true);
            let stopped = runtime.await;
            signal.and(stopped)
        }
    };
    tracing::info!(
        name="braid.runtime.stop",
        braid.runtime.generation=%generation,
        braid.runtime.stop_confirmed=result.is_ok(),
        braid.runtime.error=result.as_ref().err().map(|error|format!("{error:#}")).as_deref().unwrap_or(""),
        "standalone runtime ended"
    );
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

pub(crate) fn initial_health(config: &Config, provider_enabled: bool) -> HealthSnapshot {
    HealthSnapshot {
        ready: !provider_enabled,
        ingress: "local".into(),
        repository: config.repository().into(),
        tunnel: "disabled",
        webhook_url: None,
        reconciliation: "local",
        provider: if provider_enabled { "starting" } else { "disabled" },
        last_error: None,
    }
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn serve_managed(
    config: Config,
    provider_enabled: bool,
    mut external_stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<()> {
    if *external_stop.borrow() {
        return Ok(());
    }
    *health.write().await = initial_health(&config, provider_enabled);
    let store = Arc::new(StoreActor::start(
        config.runtime.database().to_path_buf(),
        config.runtime.backups().to_path_buf(),
    )?);
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
    store.recover_writes()?;
    let local = config.local.as_ref().context("local collaboration configuration missing")?;
    let repository =
        git2::Repository::open(&local.source).context("local source is not a Git checkout")?;
    if repository.head_detached()? {
        bail!("local source requires a named checkout branch");
    }
    repository
        .find_branch(&local.default_branch, git2::BranchType::Local)
        .with_context(|| {
            format!("configured local default branch {} does not exist", local.default_branch)
        })?
        .get()
        .peel_to_commit()
        .context("configured local default branch does not resolve to a commit")?;
    let default_branch = local.default_branch.clone();
    drop(repository);
    let local_objects = LocalObjects::open(config.runtime.database(), config.repository())?;
    local_objects.bind_source(&local.source)?;
    let objects = Arc::new(Mutex::new(local_objects));
    let backend =
        Arc::new(CollaborationBackend::Local { objects: Arc::clone(&objects), default_branch });
    let mut provider_groups = Vec::new();
    if provider_enabled {
        let factories = crate::provider::session_factories(&config)?;
        for kind in [GroupKind::Issue, GroupKind::Pr] {
            let spec = GroupSpec::new(kind, &config, &store)?;
            let factory = factories
                .get(spec.profile_id())
                .context("Profile session factory missing")?
                .clone();
            provider_groups.push((spec, factory));
        }
    }
    let (shutdown, receiver) = watch::channel(false);
    let health_server =
        crate::runtime::spawn_health(config.server.health, Arc::clone(&health), receiver.clone())
            .await?;
    let mut workers = JoinSet::new();
    workers.spawn(worker_result(queue_worker(Arc::clone(&store), receiver.clone())));
    workers.spawn(worker_result(crate::producer::lease_worker(
        Arc::clone(&store),
        lease,
        receiver.clone(),
    )));
    workers.spawn(object_worker(Arc::clone(&store), objects, config.clone(), receiver.clone()));
    if provider_enabled {
        let (reports, report_receiver) = tokio::sync::mpsc::channel(8);
        workers.spawn(worker_result(crate::health::provider_health_worker(
            Arc::clone(&health),
            report_receiver,
            receiver.clone(),
        )));
        for (spec, factory) in provider_groups {
            workers.spawn(agent_group_worker_backend(
                Arc::clone(&store),
                Arc::clone(&backend),
                config.clone(),
                spec,
                factory,
                reports.clone(),
                receiver.clone(),
            ));
        }
        drop(reports);
    }
    tracing::info!(repository = %config.repository(), "Braid local collaboration runtime is ready");
    let result = tokio::select! {
        _ = external_stop.changed() => Ok(()),
        worker = workers.join_next() => match worker {
            Some(Ok(Err(error))) => Err(error.context("local collaboration worker failed")),
            Some(Err(error)) => Err(anyhow::Error::new(error).context("local runtime worker failed")),
            Some(Ok(Ok(()))) => Err(anyhow::anyhow!("local runtime worker stopped unexpectedly")),
            None => Err(anyhow::anyhow!("all local runtime workers stopped unexpectedly")),
        },
    };
    {
        let mut snapshot = health.write().await;
        snapshot.ready = false;
        if let Err(error) = &result {
            snapshot.last_error = Some(format!("{error:#}"));
        }
    }
    let _ = shutdown.send(true);
    let mut failures = Vec::new();
    if let Err(error) = result {
        failures.push(format!("{error:#}"));
    }
    while let Some(worker) = workers.join_next().await {
        match worker {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failures.push(format!("{error:#}")),
            Err(error) => failures.push(format!("runtime worker failed during shutdown: {error}")),
        }
    }
    if let Err(error) = health_server.await {
        failures.push(format!("local health server failed: {error}"));
    }
    let mut snapshot = health.write().await;
    snapshot.ready = false;
    let generation = crate::telemetry::evidence_health().runtime_generation;
    tracing::info!(
        name="braid.runtime.stop",
        braid.runtime.generation=generation.as_deref().unwrap_or(""),
        braid.runtime.stop_confirmed=failures.is_empty(),
        braid.runtime.ready=false,
        braid.runtime.error=%failures.join("\n"),
        "managed local runtime stopped"
    );
    if failures.is_empty() {
        Ok(())
    } else {
        let error = failures.join("\n");
        snapshot.last_error = Some(error.clone());
        bail!(error)
    }
}

async fn object_worker(
    store: Arc<StoreActor>,
    objects: Arc<Mutex<LocalObjects>>,
    config: Config,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tick.tick() => {
                advance_objects(&store, &objects, &config).context("cannot advance local collaboration")?;
            }
        }
    }
}

fn advance_objects(
    store: &StoreActor,
    objects: &Mutex<LocalObjects>,
    config: &Config,
) -> Result<()> {
    let mut objects =
        objects.lock().map_err(|_| anyhow::anyhow!("local object store lock poisoned"))?;
    for pending in objects.pending_events(100)? {
        let delivery_guid = pending.event.delivery_guid.clone();
        let number = pending.event.work_item_number;
        let item_kind = pending.event.work_item_kind;
        let queue_kind = pending.event.kind.as_str();
        let generation = crate::telemetry::evidence_health().runtime_generation;
        let ingest = store.ingest_event(pending.event, policy_from_config(config));
        tracing::info!(
            name="braid.local.event_ingest",
            braid.runtime.generation=generation.as_deref().unwrap_or(""),
            braid.local.event_id=pending.event_id,
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
            "local collaboration event ingested"
        );
        ingest?;
        let ack = objects.ack_event(pending.event_id);
        tracing::info!(
            name="braid.local.event_ack",
            braid.runtime.generation=generation.as_deref().unwrap_or(""),
            braid.local.event_id=pending.event_id,
            braid.delivery.guid=%delivery_guid,
            braid.work_item.number=number,
            braid.work_item.kind=item_kind.unwrap_or(""),
            braid.queue.kind=queue_kind,
            braid.event.acked=ack.is_ok(),
            braid.event.error=ack.as_ref().err().map(|error|format!("{error:#}")).as_deref().unwrap_or(""),
            "local collaboration event acknowledged"
        );
        ack?;
    }
    if let Some(write) = store.claim_github_write()? {
        match objects.apply_outbox(&write) {
            Ok((database_id, node_id)) => {
                tracing::info!(
                    name="braid.local.outbox_result",
                    braid.runtime.generation=crate::telemetry::evidence_health().runtime_generation.as_deref().unwrap_or(""),
                    braid.outbox.intent_id=%write.intent_id,
                    braid.outbox.operation=%write.operation,
                    braid.outbox.lifecycle="applied",
                    "local collaboration write applied"
                );
                store.finish_github_write(
                    write.intent_id,
                    "applied",
                    database_id,
                    node_id,
                    None,
                )?;
            }
            Err(error) => {
                let lifecycle = if error.downcast_ref::<rusqlite::Error>().is_some_and(|error| {
                    matches!(error, rusqlite::Error::SqliteFailure(code, _) if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked | rusqlite::ErrorCode::SystemIoFailure))
                }) { "uncertain" } else { "rejected" };
                tracing::error!(
                    name="braid.local.outbox_result",
                    braid.runtime.generation=crate::telemetry::evidence_health().runtime_generation.as_deref().unwrap_or(""),
                    braid.outbox.intent_id=%write.intent_id,
                    braid.outbox.operation=%write.operation,
                    braid.outbox.lifecycle=lifecycle,
                    braid.outbox.error=%format!("{error:#}"),
                    "local collaboration write failed"
                );
                store.finish_github_write(
                    write.intent_id,
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
