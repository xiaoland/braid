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
    external_stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
) -> Result<()> {
    managed(config, provider_enabled, external_stop, health, None).await.map(|_| ())
}

pub(crate) async fn finite_managed(
    config: Config,
    request: crate::run::RunRequest,
    store: Arc<StoreActor>,
    external_stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
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

async fn managed(
    config: Config,
    provider_enabled: bool,
    mut external_stop: watch::Receiver<bool>,
    health: Arc<RwLock<HealthSnapshot>>,
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
    *health.write().await = initial_health(&config, provider_enabled);
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
    let mut outcome = None;
    let result = tokio::select! {
        _ = external_stop.changed() => {
            if let Some((request,_))=&finite {
                outcome=Some(crate::run::RunResult {run_id:request.run_id.clone(),root_issue:config.finite_run.as_ref().expect("finite context").root,state:crate::run::RunState::Stopped,delivery_commit:None,reason:Some("external stop requested".into()),stop_confirmed:false});
            }
            Ok(())
        },
        next = finite_outcome(&config,&client,&store,&health), if finite.is_some() => {
            match next {Ok(result)=> {outcome=Some(result);Ok(())}, Err(error)=>Err(error)}
        },
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
    let runtime_error = result.err().map(|error| format!("{error:#}"));
    let mut failures = Vec::new();
    let cleanup = async {
        while let Some(worker) = workers.join_next().await {
            match worker {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(format!("{error:#}")),
                Err(error) => {
                    failures.push(format!("runtime worker failed during shutdown: {error}"))
                }
            }
        }
    };
    if finite.is_some() {
        if tokio::time::timeout(Duration::from_secs(15), cleanup).await.is_err() {
            failures.push("finite runtime shutdown exceeded its deadline; physical execution stop is unconfirmed".into());
            workers.abort_all();
            while workers.join_next().await.is_some() {}
        }
    } else {
        cleanup.await;
    }
    if finite.is_some() {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let status = match store.runtime_status() {
                Ok(status) => status,
                Err(error) => {
                    failures.push(format!("shutdown ledger status failed: {error:#}"));
                    break;
                }
            };
            if status.pending_writes == 0 {
                break;
            }
            if let Err(error) = advance_objects(&store, &client, &config).await {
                if let Some(result) = &mut outcome {
                    result.state = crate::run::RunState::Blocked;
                    result.reason = Some(format!("shutdown outbox failed: {error:#}"));
                }
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let status = store.runtime_status();
        if let Ok(status) = status {
            if status.pending_writes > 0 || status.uncertain_writes > 0 {
                if let Some(result) = &mut outcome {
                    result.state = crate::run::RunState::Blocked;
                    result.reason = Some(format!(
                        "shutdown left {} pending and {} uncertain platform writes",
                        status.pending_writes, status.uncertain_writes
                    ));
                }
            }
        } else if let Err(error) = status {
            failures.push(format!("shutdown ledger status failed: {error:#}"));
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
    if let Some((request, _)) = &finite {
        let mut result = outcome.unwrap_or(crate::run::RunResult {
            run_id: request.run_id.clone(),
            root_issue: config.finite_run.as_ref().expect("finite context").root,
            state: crate::run::RunState::Failed,
            delivery_commit: None,
            reason: runtime_error.clone(),
            stop_confirmed: false,
        });
        result.stop_confirmed = failures.is_empty();
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
    let scope = if let Some(run) = &config.finite_run
        && !feed.events.is_empty()
    {
        let scope: crate::platform::objects::ScopeSnapshot =
            client.get(&format!("api/platform/scope/{}", run.root)).await?;
        anyhow::ensure!(
            scope.root.number == run.root
                && scope.run_id.as_deref() == Some(&run.run_id)
                && scope.client_id.as_deref() == Some(&client.binding.client_id)
                && scope.delivery_ref.as_deref() == Some(&run.delivery_ref),
            "platform finite-run identity or delivery policy changed"
        );
        if scope.root.state != "open" {
            run.admission.close();
        }
        Some(
            scope
                .issues
                .iter()
                .chain(&scope.pull_requests)
                .map(|item| item.number)
                .collect::<std::collections::HashSet<_>>(),
        )
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

async fn finite_outcome(
    config: &Config,
    client: &BuiltinClient,
    store: &StoreActor,
    health: &RwLock<HealthSnapshot>,
) -> Result<crate::run::RunResult> {
    let run = config.finite_run.as_ref().context("finite context missing")?;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut unavailable_since = None;
    loop {
        tick.tick().await;
        let scope: crate::platform::objects::ScopeSnapshot =
            client.get(&format!("api/platform/scope/{}", run.root)).await?;
        anyhow::ensure!(
            scope.root.number == run.root
                && scope.run_id.as_deref() == Some(&run.run_id)
                && scope.client_id.as_deref() == Some(&client.binding.client_id)
                && scope.delivery_ref.as_deref() == Some(&run.delivery_ref),
            "platform finite-run identity or delivery policy changed"
        );
        if scope.root.state != "open" {
            run.admission.close();
        }
        let status = store.runtime_status()?;
        if scope.root.state == "open" {
            let readiness = health.read().await.clone();
            if readiness.provider == "unavailable" && readiness.last_error.is_some() {
                let since = unavailable_since.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() >= Duration::from_secs(6) {
                    return Ok(crate::run::RunResult {
                        run_id: run.run_id.clone(),
                        root_issue: run.root,
                        state: crate::run::RunState::Blocked,
                        delivery_commit: None,
                        reason: Some(format!(
                            "provider remained unavailable through recovery attempts: {}",
                            readiness.last_error.as_deref().unwrap_or("no diagnostic supplied")
                        )),
                        stop_confirmed: false,
                    });
                }
            } else {
                unavailable_since = None;
            }
            if run.admission.is_closed() {
                return Ok(crate::run::RunResult {run_id:run.run_id.clone(),root_issue:run.root,state:crate::run::RunState::Blocked,delivery_commit:None,reason:Some("execution admission fenced after an unconfirmed physical stop; inspect retained session stop evidence".into()),stop_confirmed:false});
            }
            if let Some(group) = status.agent_groups.iter().find(|group| {
                group.assignment_lifecycle == "blocked"
                    || group.session_lifecycle.as_deref() == Some("blocked")
            }) {
                return Ok(crate::run::RunResult {run_id:run.run_id.clone(),root_issue:run.root,state:crate::run::RunState::Blocked,delivery_commit:None,reason:Some(format!("{} #{} execution is blocked: {}",group.work_item_kind,group.work_item_number,group.context_error.as_deref().unwrap_or("provider or assignment could not recover; inspect retained execution error"))),stop_confirmed:false});
            }
            if let Some(reset) =
                status.context_resets.iter().find(|reset| reset.lifecycle == "blocked")
            {
                return Ok(crate::run::RunResult {
                    run_id: run.run_id.clone(),
                    root_issue: run.root,
                    state: crate::run::RunState::Blocked,
                    delivery_commit: None,
                    reason: Some(format!(
                        "context reset {} is blocked; inspect retained reset error",
                        reset.reset_id
                    )),
                    stop_confirmed: false,
                });
            }
            continue;
        }
        let mut result = crate::run::RunResult {
            run_id: run.run_id.clone(),
            root_issue: run.root,
            state: crate::run::RunState::Blocked,
            delivery_commit: None,
            reason: None,
            stop_confirmed: false,
        };
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
        let status = store.runtime_status()?;
        if status.uncertain_writes > 0 {
            result.reason = Some(
                "root closed with uncertain platform writes; inspect retained outbox receipts"
                    .into(),
            );
            return Ok(result);
        }
        if status.context_resets.iter().any(|reset| reset.lifecycle != "applied") {
            result.reason=Some("root closed with an unresolved context reset; no replacement execution is admitted".into());
            return Ok(result);
        }
        if status.pending_writes > 0 {
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
