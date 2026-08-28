use sha2::{Digest, Sha256};
use super::*;
use crate::runtime::provider::agent_attributions;
use crate::webhook;

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

pub(crate) async fn reconcile_once(
    store: &StoreActor,
    github: &GitHubClient,
    config: &Config,
) -> Result<usize> {
    if store.tracked_work_items()?.is_empty() {
        return Ok(0);
    }
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

pub(crate) async fn reconcile_work_items(
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
            config.profile(&config.profile_selection.default_pr_profile)?
        } else {
            config
                .profiles
                .iter()
                .find(|profile| profile.has_tag("issue"))
                .context("configuration has no Issue Profile")?
        };
        let rendered = context::render_complete(
            &canonical,
            profile.github_context_soft_ratio,
            profile.github_context_hard_bytes,
        );
        context::record_context_revision(&canonical, &rendered, store)?;
    }
    Ok((tracked.len(), changes))
}

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
    let attributions = agent_attributions(config);
    let mut changes = 0;
    for observation in current {
        let previous = prior.get(&observation.object_node_id);
        if previous.is_some_and(|previous| observation_unchanged(previous, observation)) {
            continue;
        }
        let (mut action, mut classification) = reconciled_change(previous, observation);
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
                let state_changed = !scope
                    .previous_work_item_state
                    .eq_ignore_ascii_case(&observation.work_item_state);
                let observed = store.has_lifecycle_observation(
                    observation.work_item_node_id.clone(),
                    lifecycle_action.into(),
                    observation.version.clone(),
                )?;
                if state_changed || observed {
                    action = lifecycle_action;
                    classification = if observed { "no_wake" } else { "lifecycle" };
                }
            }
        }
        let external = observation_is_external(
            observation,
            github.identity().actor_node_id.as_str(),
            config,
            &attributions,
        );
        let cross_surface_invalidation =
            external && observation.object_kind == "issue" && action == "edited";
        let event = reconciled_event(
            observation,
            action,
            classification,
            external,
            cross_surface_invalidation,
            config,
        );
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
            visible_body: None,
        };
        let event =
            reconciled_event(&observation, "deleted", "hard_invalidation", true, false, config);
        if store.ingest_event(event, policy)?.event_id.is_some() {
            changes += 1;
        }
    }
    Ok(changes)
}

pub(crate) fn observation_is_external(
    observation: &CanonicalObservation,
    app_actor_node_id: &str,
    config: &Config,
    attributions: &[String],
) -> bool {
    let profile_origin = observation.author_node_id.as_deref().is_some_and(|author_node_id| {
        config
            .profiles
            .iter()
            .any(|profile| profile.github_actor_node_id.as_deref() == Some(author_node_id))
    });
    observation.author_node_id.as_deref() != Some(app_actor_node_id)
        && !profile_origin
        && !observation
            .body
            .as_deref()
            .is_some_and(|body| webhook::has_agent_attribution(body, attributions))
}

pub(crate) fn observation_unchanged(
    previous: &CanonicalObjectState,
    observation: &CanonicalObservation,
) -> bool {
    if previous.lifecycle != observation.lifecycle {
        return false;
    }
    if matches!(observation.object_kind, "issue" | "pr") {
        previous.version == observation.version || previous.digest == observation.digest
    } else {
        previous.version == observation.version && previous.digest == observation.digest
    }
}

pub(crate) fn reconciled_change(
    previous: Option<&CanonicalObjectState>,
    observation: &CanonicalObservation,
) -> (&'static str, &'static str) {
    let restored = previous.is_some_and(|previous| {
        previous.lifecycle == "minimized" && observation.lifecycle == "active"
    });
    match observation.object_kind {
        "issue" | "pr" | "review_thread" if previous.is_none() => ("observed", "no_wake"),
        "review_thread"
            if previous.is_some_and(|previous| {
                previous.lifecycle == observation.lifecycle
                    && previous.version.starts_with("webhook:")
            }) =>
        {
            ("observed", "no_wake")
        }
        "review" if previous.is_none() => ("submitted", "wake"),
        "review" if observation.lifecycle == "dismissed" => ("dismissed", "hard_invalidation"),
        "review_thread" if observation.lifecycle == "resolved" => ("resolved", "hard_invalidation"),
        "review_thread" if previous.is_some_and(|previous| previous.lifecycle == "resolved") => {
            ("unresolved", "wake")
        }
        _ if previous.is_none() => ("created", "wake"),
        _ if restored => ("unminimized", "wake"),
        _ if observation.lifecycle == "minimized" => ("minimized", "hard_invalidation"),
        _ => ("edited", "hard_invalidation"),
    }
}

pub(crate) fn reconciled_event(
    observation: &CanonicalObservation,
    action: &'static str,
    classification: &'static str,
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
        visible_body: observation.visible_body.clone(),
        actor_node_id: observation.author_node_id.clone(),
        actor_login: observation.author_login.clone(),
        classification: if external { classification } else { "agent_origin" },
        cross_surface_invalidation,
        origin: if external { "reconciliation" } else { "agent" },
        reference: webhook::event_reference(
            event_name,
            Some(action),
            &observation.repository,
            Some(observation.work_item_kind),
            Some(observation.work_item_number),
            (!observation.database_id.is_empty()).then_some(observation.database_id.as_str()),
            observation.author_login.as_deref(),
        ),
        mention_candidate,
        reaction_target,
        known: true,
        raw_payload: raw,
    }
}

pub(crate) struct RunningAgentTurn {
    pub(crate) claim: TurnClaim,
    pub(crate) provider_turn_id: String,
    pub(crate) reset_id: Option<String>,
}
