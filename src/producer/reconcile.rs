#![allow(clippy::large_futures)]
use std::{
    collections::{BTreeMap, HashMap},
    sync::{Arc, Mutex as StdMutex},
};

use anyhow::Result;
use sha2::{Digest, Sha256};
use tokio::{
    sync::{RwLock, watch},
    time::{Duration, MissedTickBehavior},
};

pub(crate) const LEASE_TTL_SECONDS: u64 = 30;

use crate::{
    config::Config,
    context::{self, CanonicalContext, CanonicalObservation},
    github::{GitHubClient, RepositoryName, WorkItemLocator},
    health::HealthSnapshot,
    store::{
        CanonicalObjectState, EventKind, IngressEvent, ReactionTarget, RuntimeLease,
        SchedulerPolicy, StoreActor, TrackedWorkItem,
    },
    webhook,
};

/// The canonical GitHub identity of one reconciled Work Item.
pub(crate) struct ReconcileScope {
    pub(crate) work_item_node_id: String,
    pub(crate) work_item_kind: &'static str,
    pub(crate) work_item_number: u64,
    pub(crate) work_item_state: String,
    pub(crate) previous_work_item_state: String,
    pub(crate) repository_node_id: String,
    pub(crate) repository: String,
}

pub(crate) async fn lease_worker(
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

pub(crate) async fn reconciliation_worker(
    store: Arc<StoreActor>,
    github: Arc<HashMap<String, Arc<GitHubClient>>>,
    config: Config,
    health: Arc<RwLock<HealthSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick =
        tokio::time::interval(Duration::from_secs(config.scheduler.reconciliation_seconds.min(5)));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                match Box::pin(reconcile_once(&store, &github, &config)).await {
                    Ok(Some(changes)) => {
                        let mut current = health.write().await;
                        current.reconciliation = "current";
                        if changes > 0 {
                            tracing::info!(changes, "canonical GitHub reconciliation observed changes");
                        }
                    }
                    // A local due check is not evidence of recovered GitHub
                    // state. Keep a previous failure visible until a traversal succeeds.
                    Ok(None) => {}
                    Err(error) => {
                        let mut current = health.write().await;
                        if current.reconciliation != "degraded/delayed" {
                            tracing::warn!(%error, "canonical GitHub reconciliation degraded/delayed");
                        }
                        current.reconciliation = "degraded/delayed";
                        current.last_error = Some(error.to_string());
                    }
                }
            }
        }
    }
}

pub(crate) async fn reconcile_once(
    store: &StoreActor,
    github: &HashMap<String, Arc<GitHubClient>>,
    config: &Config,
) -> Result<Option<usize>> {
    let tracked = store.due_reconciliation_work_items(config.scheduler.reconciliation_seconds)?;
    let mut attempted = false;
    let mut changes = 0;
    let mut failures = Vec::new();
    let mut delayed_installations = std::collections::HashSet::new();
    // Global least-recently-attempted order, not a fixed repository/first-item order.
    for work_item in &tracked {
        let Some(client) = github.get(&work_item.repository) else { continue };
        if delayed_installations.contains(&client.identity().installation_id) {
            continue;
        }
        if let Some(reason) = client.reconciliation_delay().await {
            delayed_installations.insert(client.identity().installation_id);
            failures.push(format!("installation {}: {reason}", client.identity().installation_id));
            continue;
        }
        let client = client.background();
        let run = match store.begin_reconciliation(
            client.identity().repository_node_id.clone(),
            work_item.repository.clone(),
        ) {
            Ok(run) => run,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };
        store.record_reconciliation(work_item.node_id.clone(), false)?;
        attempted = true;
        match Box::pin(reconcile_work_item(store, &client, config, work_item)).await {
            Ok(item_changes) => {
                store.record_reconciliation(work_item.node_id.clone(), true)?;
                store.finish_reconciliation(run, "completed", 1, item_changes, None)?;
                changes += item_changes;
            }
            Err(error) => {
                if error.chain().any(|cause| {
                    cause.downcast_ref::<crate::github::GitHubError>().is_some_and(|error| {
                        matches!(error, crate::github::GitHubError::Delayed(_))
                    })
                }) {
                    delayed_installations.insert(client.identity().installation_id);
                }
                store.finish_reconciliation(run, "failed", 0, 0, Some(error.to_string()))?;
                failures.push(format!("{}#{}: {error:#}", work_item.repository, work_item.number));
            }
        }
    }
    if failures.is_empty() {
        Ok(attempted.then_some(changes))
    } else {
        anyhow::bail!(failures.join("; "))
    }
}

async fn reconcile_work_item(
    store: &StoreActor,
    github: &GitHubClient,
    config: &Config,
    work_item: &TrackedWorkItem,
) -> Result<usize> {
    let mut changes = 0;
    let repository = work_item.repository.parse::<RepositoryName>()?;
    let locator = WorkItemLocator { repository, number: work_item.number };
    let mut canonical = if work_item.kind == "issue" {
        CanonicalContext::Issue(context::materialize_issue(github, &locator, 100).await?)
    } else if work_item.kind == "pr" {
        CanonicalContext::PullRequest(
            context::materialize_pull_request(github, &locator, 100).await?,
        )
    } else {
        return Ok(0);
    };
    let prior = store
        .processed_objects(work_item.node_id.clone())?
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
            previous_work_item_state: work_item.state.clone(),
            repository_node_id: issue.repository_node_id.clone(),
            repository: issue.repository.clone(),
        },
        CanonicalContext::PullRequest(pull_request) => ReconcileScope {
            work_item_node_id: pull_request.node_id.clone(),
            work_item_kind: "pr",
            work_item_number: pull_request.number,
            work_item_state: context::pull_request_work_item_state(pull_request),
            previous_work_item_state: work_item.state.clone(),
            repository_node_id: pull_request.repository_node_id.clone(),
            repository: pull_request.repository.clone(),
        },
    };
    let observations = context::canonical_observations(&canonical);
    changes += reconcile_observations(store, github, config, &scope, &prior, &observations)?;
    context::reconcile_local_state(&mut canonical, store)?;
    let profile = if work_item.kind == "pr" {
        config.pr_profile_for_repository(&work_item.repository)?
    } else {
        config.issue_profile_for_repository(&work_item.repository)?
    };
    let rendered = context::render_complete(
        &canonical,
        profile.github_context_soft_ratio,
        profile.github_context_hard_bytes,
    );
    context::record_context_revision(&canonical, &rendered, store)?;
    Ok(changes)
}

#[allow(clippy::too_many_lines)]
pub(crate) fn reconcile_observations(
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
    let mut events = Vec::new();
    let mut processed = current
        .iter()
        .map(|observation| CanonicalObjectState {
            node_id: observation.object_node_id.clone(),
            database_id: Some(observation.database_id.clone()),
            object_kind: observation.object_kind.into(),
            version: observation.version.clone(),
            digest: observation.digest.clone(),
            lifecycle: if matches!(observation.object_kind, "issue" | "pr") {
                observation.work_item_state.to_ascii_lowercase()
            } else {
                observation.lifecycle.into()
            },
            author_node_id: observation.author_node_id.clone(),
            author_login: observation.author_login.clone(),
            metadata_digest: observation.metadata_digest.clone(),
        })
        .collect::<Vec<_>>();
    for observation in current {
        let previous = prior.get(&observation.object_node_id);
        // A work-item state transition (close/reopen/merge) must never be
        // skipped: the body digest often survives it, and the issue/pr
        // unchanged rule intentionally treats digest-equal as unchanged for
        // ordinary comment-bump noise.
        let previous_state =
            previous.map_or(scope.previous_work_item_state.as_str(), |p| p.lifecycle.as_str());
        let state_transition = matches!(observation.object_kind, "issue" | "pr")
            && !previous_state.eq_ignore_ascii_case(&observation.work_item_state);
        if !state_transition
            && previous.is_some_and(|previous| observation_unchanged(previous, observation))
        {
            if previous.is_some_and(|p| p.metadata_digest.is_none())
                && observation.metadata_digest.is_some()
            {
                let mut epoch =
                    reconciled_event(observation, "observed", EventKind::Noop, true, false, config);
                epoch.identity = crate::store::ChangeIdentity::Occurrence;
                epoch.delivery_guid = format!("reconcile-epoch-{}", uuid::Uuid::now_v7());
                events.push(epoch);
            }
            continue;
        }
        let (mut action, mut kind) = reconciled_change(previous, observation);
        if matches!(observation.object_kind, "issue" | "pr") {
            let lifecycle_action = if matches!(
                observation.work_item_state.to_ascii_lowercase().as_str(),
                "closed" | "merged"
            ) {
                Some("closed")
            } else if observation.work_item_state.eq_ignore_ascii_case("open") {
                Some("reopened")
            } else {
                None
            };
            if let Some(lifecycle_action) = lifecycle_action {
                let state_changed =
                    !previous_state.eq_ignore_ascii_case(&observation.work_item_state);
                let observed = store.has_lifecycle_observation(
                    observation.work_item_node_id.clone(),
                    lifecycle_action.into(),
                    observation.version.clone(),
                )?;
                if state_changed || observed {
                    action = lifecycle_action;
                    kind = if observed { EventKind::Noop } else { EventKind::Lifecycle };
                }
            }
        }
        let metadata_changed =
            previous.is_some_and(|p| p.metadata_digest != observation.metadata_digest);
        // A title/body publication proves only those fields, never a later
        // metadata or native-edge change sharing a coarse root timestamp.
        let external = metadata_changed
            || observation_is_external(
                store,
                observation,
                action,
                github.identity().actor_node_id.as_str(),
                config,
            )?;
        let cross_surface_invalidation =
            external && observation.object_kind == "issue" && action == "edited";
        let mut event = reconciled_event(
            observation,
            action,
            kind,
            external,
            cross_surface_invalidation,
            config,
        );
        if metadata_changed {
            event.identity = crate::store::ChangeIdentity::Occurrence;
            event.delivery_guid = format!("reconcile-transition-{}", uuid::Uuid::now_v7());
            event.object_digest = Some(hex::encode(Sha256::digest(
                format!(
                    "braid-metadata-change-v1\0{}\0{}",
                    observation.digest,
                    observation.metadata_digest.as_deref().unwrap_or("")
                )
                .as_bytes(),
            )));
        }
        if observation.object_kind == "review_thread"
            && !metadata_changed
            && previous.is_some_and(|p| p.lifecycle != observation.lifecycle)
            && matches!(action, "resolved" | "unresolved")
        {
            event.identity = crate::store::ChangeIdentity::ObservedTransition;
        }
        events.push(event);
    }
    for previous in prior.values() {
        if current_map.contains_key(previous.node_id.as_str()) {
            continue;
        }
        if previous.lifecycle == "deleted" {
            processed.push(previous.clone());
            continue;
        }
        let observation = CanonicalObservation {
            unedited_creation: false,
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
            visible_body: None,
            title: None,
            metadata_digest: None,
        };
        let mut deleted = previous.clone();
        deleted.version.clone_from(&observation.version);
        deleted.lifecycle = "deleted".into();
        processed.push(deleted);
        events.push(reconciled_event(
            &observation,
            "deleted",
            EventKind::Invalidate,
            true,
            false,
            config,
        ));
    }
    store
        .commit_observation(crate::store::ObservationCommit {
            work_item_node_id: scope.work_item_node_id.clone(),
            expected: prior.values().cloned().collect(),
            current: processed,
            events,
            policy,
        })
        .map_err(Into::into)
}

pub(crate) fn observation_is_external(
    store: &StoreActor,
    observation: &CanonicalObservation,
    action: &str,
    app_actor_node_id: &str,
    config: &Config,
) -> Result<bool> {
    let fingerprint = observation.version_digest();
    if store.is_agent_version(
        observation.object_node_id.clone(),
        observation.version.clone(),
        fingerprint,
    )? {
        return Ok(false);
    }
    // The creator is evidence for creation only, never for a later Human edit.
    if !current_actor_known(observation, action) {
        return Ok(true);
    }
    let profile_origin = observation.author_node_id.as_deref().is_some_and(|author_node_id| {
        config
            .profiles
            .iter()
            .any(|profile| profile.github_actor_node_id.as_deref() == Some(author_node_id))
    });
    Ok(observation.author_node_id.as_deref() != Some(app_actor_node_id) && !profile_origin)
}

fn current_actor_known(observation: &CanonicalObservation, action: &str) -> bool {
    (observation.unedited_creation && matches!(action, "created" | "submitted"))
        || (observation.object_kind == "review_thread"
            && action == "resolved"
            && observation.author_node_id.is_some())
}

pub(crate) fn observation_unchanged(
    previous: &CanonicalObjectState,
    observation: &CanonicalObservation,
) -> bool {
    if matches!(observation.object_kind, "issue" | "pr") {
        previous.lifecycle.eq_ignore_ascii_case(&observation.work_item_state)
            && previous.digest == observation.digest
            && (previous.metadata_digest.is_none()
                || previous.metadata_digest == observation.metadata_digest)
    } else {
        previous.lifecycle == observation.lifecycle
            && (previous.digest == observation.digest
                || (previous.metadata_digest.is_none()
                    && previous.digest == observation.version_digest()))
    }
}

pub(crate) fn reconciled_change(
    previous: Option<&CanonicalObjectState>,
    observation: &CanonicalObservation,
) -> (&'static str, EventKind) {
    let restored = previous.is_some_and(|previous| {
        previous.lifecycle == "minimized" && observation.lifecycle == "active"
    });
    match observation.object_kind {
        "issue" | "pr" | "review_thread" if previous.is_none() => ("observed", EventKind::Noop),
        "review_thread"
            if previous.is_some_and(|previous| {
                previous.lifecycle == observation.lifecycle
                    && previous.version.starts_with("webhook:")
            }) =>
        {
            ("observed", EventKind::Noop)
        }
        "review" if previous.is_none() => ("submitted", EventKind::Wake),
        "review" if observation.lifecycle == "dismissed" => ("dismissed", EventKind::Invalidate),
        "review_thread"
            if observation.lifecycle == "resolved"
                && previous.is_some_and(|p| p.lifecycle != "resolved") =>
        {
            ("resolved", EventKind::Invalidate)
        }
        "review_thread" if previous.is_some_and(|previous| previous.lifecycle == "resolved") => {
            ("unresolved", EventKind::Wake)
        }
        _ if previous.is_none() => ("created", EventKind::Wake),
        _ if restored => ("unminimized", EventKind::Wake),
        _ if observation.lifecycle == "minimized" => ("minimized", EventKind::Invalidate),
        _ => ("edited", EventKind::Invalidate),
    }
}

pub(crate) fn reconciled_event(
    observation: &CanonicalObservation,
    action: &'static str,
    kind: EventKind,
    external: bool,
    cross_surface_invalidation: bool,
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
        && observation.unedited_creation
        && matches!(action, "created" | "edited" | "unminimized")
        && observation.body.as_deref().is_some_and(|body| {
            config
                .github
                .as_ref()
                .is_some_and(|github| webhook::has_visible_mention(body, &github.handle))
        });
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
    let occurrence =
        observation.object_kind == "review_thread" || matches!(action, "minimized" | "unminimized");
    let delivery_guid = if occurrence {
        format!("reconcile-transition-{}", uuid::Uuid::now_v7())
    } else {
        format!(
            "reconcile-{}",
            &hex::encode(Sha256::digest(
                format!(
                    "{}\0{}\0{}\0{}",
                    observation.object_node_id, observation.version, observation.digest, action
                )
                .as_bytes()
            ))[..32]
        )
    };
    IngressEvent {
        identity: if occurrence {
            crate::store::ChangeIdentity::Occurrence
        } else {
            crate::store::ChangeIdentity::ObjectVersion
        },
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
        object_digest: Some(observation.version_digest()),
        visible_body: observation.visible_body.clone(),
        actor_node_id: observation
            .author_node_id
            .clone()
            .filter(|_| current_actor_known(observation, action)),
        actor_login: observation
            .author_login
            .clone()
            .filter(|_| current_actor_known(observation, action)),
        kind: if external { kind } else { EventKind::OriginEcho },
        detail: Some(action),
        cross_surface_invalidation,
        requires_attention: external && matches!(kind, EventKind::Wake | EventKind::Invalidate),
        origin: if external { "reconciliation" } else { "agent" },
        reference: webhook::event_reference(
            event_name,
            Some(action),
            &observation.repository,
            Some(observation.work_item_kind),
            Some(observation.work_item_number),
            (!observation.database_id.is_empty()).then_some(observation.database_id.as_str()),
            observation
                .author_login
                .as_deref()
                .filter(|_| current_actor_known(observation, action)),
        ),
        mention_candidate,
        reaction_target,
        known: true,
        raw_payload: raw,
    }
}
