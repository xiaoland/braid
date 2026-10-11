//! Built-in upstream event bridge and execution lifecycle, backed by the Harness ledger.
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result, bail};
use tokio::{
    sync::{RwLock, watch},
    task::JoinSet,
    time::{Duration, MissedTickBehavior},
};

use crate::{
    builtin::{BuiltinClient, EventFeed},
    collaboration::CollaborationBackend,
    config::Config,
    group::{GroupKind, GroupSpec, agent_group_worker_backend},
    health::HealthSnapshot,
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
        reconciliation: "builtin",
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
    let client = BuiltinClient::connect(&config, Arc::clone(&store)).await?;
    client
        .revoke_all()
        .await
        .context("cannot revoke old built-in writer grants before execution")?;
    let backend = Arc::new(CollaborationBackend::Builtin(Arc::clone(&client)));
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
    workers.spawn(object_worker(
        Arc::clone(&store),
        Arc::clone(&client),
        config.clone(),
        receiver.clone(),
    ));
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
    tracing::info!(repository = %config.repository(), "Braid built-in Harness runtime is ready");
    let result = tokio::select! {
        _ = external_stop.changed() => Ok(()),
        worker = workers.join_next() => match worker {
            Some(Ok(Err(error))) => Err(error.context("built-in collaboration worker failed")),
            Some(Err(error)) => Err(anyhow::Error::new(error).context("built-in Harness worker failed")),
            Some(Ok(Ok(()))) => Err(anyhow::anyhow!("built-in Harness worker stopped unexpectedly")),
            None => Err(anyhow::anyhow!("all built-in Harness workers stopped unexpectedly")),
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
    if let Err(error) = client.revoke_all().await {
        failures.push(format!("built-in writer stop remains unconfirmed: {error:#}"));
    }
    if let Err(error) = health_server.await {
        failures.push(format!("Harness health server failed: {error}"));
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
        "built-in Harness runtime stopped"
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
    client: Arc<BuiltinClient>,
    config: Config,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return Ok(()),
            _ = tick.tick() => {
                advance_objects(&store, &client, &config).await.context("cannot advance built-in collaboration")?;
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

#[allow(clippy::too_many_lines)]
async fn advance_objects(
    store: &StoreActor,
    client: &BuiltinClient,
    config: &Config,
) -> Result<()> {
    let feed: EventFeed = client
        .get(&format!("api/platform/events?after={}&limit=100", client_cursor(store)?))
        .await?;
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
        client
            .reconcile_fence(&event)
            .await
            .context("cannot reconcile upstream writer revocation")?;
        let delivery_guid = event.delivery_guid.clone();
        let number = event.work_item_number;
        let item_kind = event.work_item_kind;
        let queue_kind = event.kind.as_str();
        let generation = crate::telemetry::evidence_health().runtime_generation;
        let ingest = store.ingest_event(event, policy_from_config(config));
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
        ingest?;
        let ack = store.builtin(crate::store::BuiltinCommand::Advance(pending.sequence));
        tracing::info!(
            name="braid.builtin.cursor_advance",
            braid.runtime.generation=generation.as_deref().unwrap_or(""),
            braid.local.event_id=pending.sequence,
            braid.delivery.guid=%delivery_guid,
            braid.work_item.number=number,
            braid.work_item.kind=item_kind.unwrap_or(""),
            braid.queue.kind=queue_kind,
            braid.event.cursor_saved=ack.is_ok(),
            braid.event.error=ack.as_ref().err().map(|error|format!("{error:#}")).as_deref().unwrap_or(""),
            "Harness upstream cursor advanced"
        );
        ack?;
        cursor = pending.sequence;
    }
    anyhow::ensure!(
        feed.next_after == cursor,
        "platform nextAfter does not match delivered event sequence"
    );
    if let Some(write) = store.claim_github_write()? {
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
                store.finish_github_write(
                    write.intent_id,
                    "applied",
                    database_id,
                    node_id,
                    None,
                )?;
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
