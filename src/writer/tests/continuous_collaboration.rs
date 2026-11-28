use super::*;
use crate::{
    context::CanonicalObservation,
    producer::reconcile::{self, ReconcileScope},
    store::{CanonicalCommentSet, CanonicalObjectState, EventKind, ObservationCommit},
};

pub(crate) fn revision(delivery: &str, body: &str) -> IngressEvent {
    IngressEvent {
        identity: crate::store::ChangeIdentity::ObjectVersion,
        delivery_guid: delivery.into(),
        event_name: "issues".into(),
        action: Some("edited".into()),
        repository_node_id: "repo".into(),
        repository: "owner/repository".into(),
        work_item_node_id: Some("item".into()),
        work_item_kind: Some("issue"),
        work_item_number: Some(1),
        work_item_state: Some("open".into()),
        object_node_id: Some("item".into()),
        object_version: Some(delivery.into()),
        object_digest: Some(helpers::digest(body)),
        visible_body: Some(body.into()),
        actor_node_id: Some("human".into()),
        actor_login: Some("human".into()),
        kind: EventKind::Invalidate,
        detail: Some("edited"),
        cross_surface_invalidation: true,
        requires_attention: true,
        origin: "external",
        reference: format!("Human revised Issue #1: {delivery}"),
        mention_candidate: false,
        reaction_target: None,
        known: true,
        raw_payload: Vec::new(),
    }
}

fn state(version: &str, digest: char) -> CanonicalObjectState {
    CanonicalObjectState {
        root_fields: None,
        metadata_digest: None,
        node_id: "item".into(),
        database_id: Some("1".into()),
        object_kind: "issue".into(),
        version: version.into(),
        digest: digest.to_string().repeat(64),
        lifecycle: "open".into(),
        author_node_id: Some("human".into()),
        author_login: Some("human".into()),
    }
}

#[tokio::test]
async fn another_context_read_cannot_acknowledge_a_revision_before_delivery() {
    let old_version = "2026-01-01T00:00:01Z";
    let revised_version = "2026-01-01T00:00:02Z";
    let f = Fixture::new(true);
    f.store
        .commit_observation(ObservationCommit {
            basis: f.store.begin_observation("item".into()).unwrap(),
            current: vec![state(old_version, 'a')],
            events: vec![],
            policy: SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
        })
        .unwrap();
    f.store
        .reconcile_comments(CanonicalCommentSet {
            repository_node_id: "repo".into(),
            repository: "owner/repository".into(),
            work_item_node_id: "item".into(),
            work_item_kind: "issue",
            work_item_number: 1,
            work_item_state: "open".into(),
            work_item_version: "2026-01-01T00:00:03Z".into(),
            work_item_digest: "b".repeat(64),
            object_kind: "issue_comment",
            comments: vec![],
        })
        .unwrap();
    assert_eq!(f.store.processed_objects("item".into()).unwrap()[0].version, old_version);
    let observation = CanonicalObservation {
        unedited_creation: false,
        created_at: None,
        work_item_node_id: "item".into(),
        work_item_kind: "issue",
        work_item_number: 1,
        work_item_state: "open".into(),
        repository_node_id: "repo".into(),
        repository: "owner/repository".into(),
        object_node_id: "item".into(),
        database_id: "1".into(),
        object_kind: "issue",
        version: revised_version.into(),
        digest: "b".repeat(64),
        lifecycle: "active",
        author_node_id: Some("human".into()),
        author_login: Some("human".into()),
        body: Some("Current revised contract".into()),
        visible_body: Some("Current revised contract".into()),
        root_edits: crate::context::RootEditEvidence::default(),
        title: Some("Design".into()),
        metadata_digest: None,
    };
    let scope = ReconcileScope {
        work_item_node_id: "item".into(),
        work_item_kind: "issue",
        work_item_number: 1,
        work_item_state: "open".into(),
        previous_work_item_state: "open".into(),
        repository_node_id: "repo".into(),
        repository: "owner/repository".into(),
    };
    let (github, task) = server(Arc::new(Mutex::new(Remote::default())), true).await;
    for expected_changes in [1, 0] {
        let basis = f.store.begin_observation("item".into()).unwrap();
        assert_eq!(
            reconcile::reconcile_observations(
                &f.store,
                &github,
                &f.config,
                &scope,
                basis,
                std::slice::from_ref(&observation)
            )
            .unwrap(),
            expected_changes
        );
    }
    assert_eq!(f.store.runtime_status().unwrap().batches.len(), 1);
    assert_eq!(
        f.db()
            .query_row(
                "SELECT count(*) FROM events WHERE kind='invalidate' AND requires_attention=1",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    task.abort();
}

#[test]
fn failed_observation_commits_neither_events_nor_frontier() {
    let f = Fixture::new(true);
    let result = f.store.commit_observation(ObservationCommit {
        basis: f.store.begin_observation("item".into()).unwrap(),
        current: vec![],
        events: vec![revision("v2", "new")],
        policy: SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
    });
    assert!(
        matches!(result, Err(crate::store::StoreError::InvalidData(ref reason)) if reason == "complete observation has no root"),
        "{result:?}"
    );
    assert_eq!(
        f.db().query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
    assert!(f.store.processed_objects("item".into()).unwrap().is_empty());
    assert!(f.store.runtime_status().unwrap().batches.is_empty());
}

#[test]
fn independent_pr_ensures_deliver_two_activations_and_retry_one_identity() {
    let f = Fixture::new(false);
    let policy = SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 };
    let profile = f.config.profile("pr-codex").unwrap();
    let owner =
        f.store.acquire_runtime_lease("runtime".into(), "activation-test".into(), 3600).unwrap();
    for number in [9, 10] {
        let payload = serde_json::to_string(&AcceptedRequest::Ensure {
            comment_id: number,
            head: Some(format!("head-{number}")),
            profile: profile.clone(),
        })
        .unwrap();
        let plan = f
            .store
            .prepare_implementation_request(NewImplementationRequest {
                root_intent_id: None,
                write: NewGhWriteIntent {
                    request_key: format!("ensure-{number}"),
                    operation: "pr_ensure",
                    repository: "owner/repository".into(),
                    target: format!("https://github.com/owner/repository/issues/comments/{number}"),
                    profile_id: profile.id.clone(),
                    role: "Operator".into(),
                    request_digest: helpers::digest(&payload),
                    payload,
                    source: None,
                },
                comment_database_id: number,
                comment_node_id: format!("comment-{number}"),
                issue_node_id: "item".into(),
                issue_number: 1,
                issue_title: "Design".into(),
                base_ref: "main".into(),
                head_ref: format!("head-{number}"),
                pr_profile_id: profile.id.clone(),
            })
            .unwrap();
        f.store
            .record_implementation_progress(
                plan.write.intent_id.clone(),
                ImplementationProgress {
                    stage: "activation_pending",
                    bootstrap_commit_sha: None,
                    pull_request_database_id: Some(number),
                    pull_request_node_id: Some(format!("pr-{number}")),
                    pull_request_number: Some(number),
                },
            )
            .unwrap();
        for _ in 0..2 {
            f.store
                .accept_pr_activation(crate::store::PrActivation {
                    intent_id: plan.write.intent_id.clone(),
                    repository_node: "repo".into(),
                    pr_node: format!("pr-{number}"),
                    pr_number: number,
                    policy,
                    owner: owner.clone(),
                    issue_basis: f.store.begin_observation("item".into()).unwrap(),
                    target_basis: f.store.begin_observation(format!("pr-{number}")).unwrap(),
                })
                .unwrap();
        }
        assert!(
            f.store
                .implementation_request_receipt(plan.write.intent_id)
                .unwrap()
                .unwrap()
                .activation_delivered
        );
    }
    let candidates =
        f.store.assignment_candidates("owner/repository".into(), "pr".into(), 10).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(
        f.db()
            .query_row("SELECT count(*) FROM events WHERE detail='pr_ensure'", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn human_edit_of_a_braid_authored_body_is_not_an_echo_or_an_urgent_mention() {
    use crate::webhook::{self, ActorPolicy, WebhookHeaders};
    use hmac::{Hmac, KeyInit as _, Mac as _};
    let body=serde_json::to_vec(&json!({"action":"edited","sender":{"login":"human","node_id":"human"},"repository":{"node_id":"repo","full_name":"owner/repository"},"issue":{"node_id":"item","number":1,"title":"Design","state":"open","body":"> **Braid Agent · Braid**\n> Issue Agent\n\nRevised requirements","updated_at":"v2"},"changes":{"body":{"from":"old requirements"}}})).unwrap();
    let mut mac = Hmac::<sha2::Sha256>::new_from_slice(b"secret").unwrap();
    mac.update(&body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let event = webhook::parse_verified(
        WebhookHeaders {
            signature: Some(&signature),
            delivery: Some("revision"),
            event: Some("issues"),
        },
        &body,
        b"secret",
        &["owner/repository".into()],
        "braid",
        ActorPolicy { app_node_id: "app", app_login: "braid[bot]", agent_node_ids: &[] },
    )
    .unwrap();
    assert_eq!(event.kind, EventKind::Invalidate);
    assert!(event.requires_attention);
    assert!(!event.mention_candidate);
    assert_eq!(event.origin, "external");
}

#[test]
fn interrupted_revision_coalesces_attention_into_one_continuation() {
    let f = Fixture::new(true);
    f.db().execute_batch("UPDATE turns SET lifecycle='running',provider_turn_id='old-native' WHERE turn_id='turn'; UPDATE provider_sessions SET lifecycle='running' WHERE session_id='session'").unwrap();
    let policy = SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 };
    f.store.ingest_event(revision("v2", "Current design"), policy).unwrap();
    let reset = f
        .store
        .begin_context_reset(Some("turn".into()), "issue".into(), "issue-codex".into())
        .unwrap()
        .unwrap();
    assert!(reset.continuation);
    f.store
        .mark_context_reset_turn_terminal(
            reset.reset_id.clone(),
            "turn".into(),
            "interrupted".into(),
        )
        .unwrap();
    f.store
        .complete_context_reset(
            reset.reset_id,
            "fresh-provider".into(),
            "new-context".into(),
            "instructions".into(),
        )
        .unwrap();
    f.store.ingest_event(revision("v2", "Current design"), policy).unwrap();
    assert_eq!(f.store.runtime_status().unwrap().batches.len(), 1);
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["fresh-provider".into()])
        .unwrap()
        .unwrap();
    assert_eq!(claim.references.len(), 1);
    assert_eq!(claim.trigger_kind, "wake_batch");
    assert!(
        f.store
            .claim_runnable_turn(
                "issue".into(),
                "issue-codex".into(),
                vec!["fresh-provider".into()]
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_additive_projection_does_not_replace_the_session_publication_fence() {
    let f = Fixture::new(true);
    f.db()
        .execute("UPDATE work_items SET context_revision='latest-projected-comments'", [])
        .unwrap();
    let mut input = revision("new-comment", "comment");
    input.kind = EventKind::Wake;
    input.cross_surface_invalidation = false;
    f.store.ingest_event(input, SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 }).unwrap();
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
        .unwrap()
        .unwrap();
    f.store.mark_turn_started(claim.turn_id.clone(), "native-next".into()).unwrap();
    let source =
        resolve_source(&f.config, None, &Invocation::Agent(claim.turn_id)).unwrap().unwrap();
    assert_eq!(source.context_revision, "context");
}

async fn observe_issue(
    f: &Fixture,
    github: &GitHubClient,
    change_metadata: bool,
) -> CanonicalObservation {
    let basis = f.store.begin_observation("item".into()).unwrap();
    let locator = "owner/repository#1".parse().unwrap();
    let issue = crate::context::materialize_issue(github, &locator, 100).await.unwrap();
    let context = crate::context::CanonicalContext::Issue(issue);
    let mut observed = crate::context::canonical_observations(&context).remove(0);
    if change_metadata {
        observed.metadata_digest = Some("d".repeat(64));
    }
    let scope = ReconcileScope {
        work_item_node_id: "item".into(),
        work_item_kind: "issue",
        work_item_number: 1,
        work_item_state: "open".into(),
        previous_work_item_state: "open".into(),
        repository_node_id: "repo".into(),
        repository: "owner/repository".into(),
    };
    reconcile::reconcile_observations(
        &f.store,
        github,
        &f.config,
        &scope,
        basis,
        std::slice::from_ref(&observed),
    )
    .unwrap();
    observed
}

#[tokio::test]
async fn root_edits_use_field_provenance_when_publication_evidence_is_unavailable() {
    const BEFORE: &str = "2026-01-01T00:00:01Z";
    const AFTER: &str = "2026-01-01T00:00:02Z";
    for (mode, expected) in [
        ("app-body", "origin_echo"),
        ("app-title-body", "origin_echo"),
        ("unknown-editor", "invalidate"),
        ("mixed-same-second", "invalidate"),
        ("missing-rename", "invalidate"),
        ("wrong-rename-preimage", "invalidate"),
        ("metadata", "invalidate"),
    ] {
        let f = Fixture::new(true);
        let mut remote = body_edit::BodyRemote::new(false);
        remote.snapshot.updated_at = BEFORE.into();
        let remote = Arc::new(Mutex::new(remote));
        let (github, task) = body_edit::body_server(remote.clone()).await;
        let previous = observe_issue(&f, &github, false).await;
        let previous_fields =
            f.store.processed_objects("item".into()).unwrap().remove(0).root_fields.unwrap();
        assert_eq!(Some(previous_fields.clone()), previous.root_fields());
        {
            let mut state = remote.lock().unwrap();
            state.snapshot.updated_at = AFTER.into();
            state.snapshot.body = Some("Revised design".into());
            let mut evidence = json!({
                "lastEditedAt": AFTER,
                "editor": if mode == "unknown-editor" { Value::Null } else { json!({"id":"app"}) },
                "titleHistory": {"nodes":[]},
            });
            if matches!(
                mode,
                "app-title-body" | "mixed-same-second" | "missing-rename" | "wrong-rename-preimage"
            ) {
                state.snapshot.title = "Renamed design".into();
                if mode != "missing-rename" {
                    evidence["titleHistory"]["nodes"] = json!([{
                        "id":"rename", "createdAt":AFTER, "currentTitle":state.snapshot.title,
                        "previousTitle":if mode == "wrong-rename-preimage" {"Unobserved title"} else {"Canonical"},
                        "actor":{"id":if mode == "mixed-same-second" {"human"} else {"app"}},
                    }]);
                }
            }
            state.issue_edit_evidence = evidence.as_object().unwrap().clone();
        }
        f.db()
            .execute_batch(
                "ALTER TABLE publication_versions RENAME TO unavailable_publication_versions;
             ALTER TABLE write_intents RENAME TO unavailable_write_intents;
             ALTER TABLE github_write_outbox RENAME TO unavailable_write_outbox;",
            )
            .unwrap();
        let current = observe_issue(&f, &github, mode == "metadata").await;
        let edited: (String, bool) = f.db().query_row(
            "SELECT kind,requires_attention FROM events WHERE detail='edited' AND object_node_id='item'",
            [], |r| Ok((r.get(0)?,r.get(1)?)),
        ).unwrap();
        assert_eq!(edited, (expected.into(), expected == "invalidate"), "{mode}");
        let processed = f.store.processed_objects("item".into()).unwrap().remove(0);
        assert_eq!(processed.root_fields, current.root_fields(), "{mode}");
        assert_ne!(previous_fields.body_digest, processed.root_fields.unwrap().body_digest);
        task.abort();
    }
}

#[tokio::test]
async fn exact_native_edit_evidence_proves_only_the_fields_its_sender_changed() {
    for (mode, expected) in [
        ("app-with-body-preimage", "origin_echo"),
        ("app-with-unproven-title", "invalidate"),
        ("human-against-app-editor", "invalidate"),
    ] {
        let f = Fixture::new(true);
        let mut remote = body_edit::BodyRemote::new(false);
        remote.snapshot.updated_at = "2026-01-01T00:00:01Z".into();
        let remote = Arc::new(Mutex::new(remote));
        let (github, task) = body_edit::body_server(remote.clone()).await;
        let previous = observe_issue(&f, &github, false).await;
        let previous_body = previous.body.unwrap();
        let update_time = "2026-01-01T00:00:02Z";
        let new_body = "Updated design";
        let title = if mode == "app-with-unproven-title" { "Human title" } else { "Canonical" };
        let human = mode == "human-against-app-editor";
        {
            let mut state = remote.lock().unwrap();
            state.snapshot.updated_at = update_time.into();
            state.snapshot.body = Some(new_body.into());
            state.snapshot.title = title.into();
            if human {
                state.issue_edit_evidence = json!({
                    "lastEditedAt": update_time,
                    "editor":{"id":"app"},
                })
                .as_object()
                .unwrap()
                .clone();
            }
        }
        let mut native = revision("native-edit", new_body);
        native.object_version = Some(update_time.into());
        native.object_digest =
            Some(crate::store::publication_digest("item", update_time, Some(title), new_body));
        native.actor_node_id = Some(if human { "human" } else { "app" }.into());
        if !human {
            native.kind = EventKind::OriginEcho;
            native.origin = "agent";
            native.requires_attention = false;
        }
        native.raw_payload = serde_json::to_vec(&json!({
            "changes":{"body":{"from":previous_body}}
        }))
        .unwrap();
        f.store
            .ingest_event(native, SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 })
            .unwrap();
        observe_issue(&f, &github, false).await;
        let kind: String = f
            .db()
            .query_row(
                "SELECT e.kind FROM deliveries d JOIN events e ON e.event_id=d.effect_event_id
             WHERE d.delivery_guid LIKE 'reconcile-%' AND d.action='edited'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, expected, "{mode}");
        task.abort();
    }
}

#[tokio::test]
async fn native_association_changes_are_visible_without_a_root_version_change() {
    let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
    let (github, http) = body_edit::body_server(remote.clone()).await;
    let locator = "owner/repository#9".parse().unwrap();
    let before = crate::context::CanonicalContext::PullRequest(
        crate::context::materialize_pull_request(&github, &locator, 100).await.unwrap(),
    );
    remote.lock().unwrap().associated = false;
    let after = crate::context::CanonicalContext::PullRequest(
        crate::context::materialize_pull_request(&github, &locator, 100).await.unwrap(),
    );
    let before = crate::context::canonical_observations(&before).remove(0);
    let after = crate::context::canonical_observations(&after).remove(0);
    assert_eq!(before.version, after.version);
    assert_eq!(before.body, after.body);
    assert_ne!(before.metadata_digest, after.metadata_digest);
    let prior = CanonicalObjectState {
        root_fields: before.root_fields(),
        node_id: before.object_node_id,
        database_id: Some(before.database_id),
        object_kind: "pr".into(),
        version: before.version,
        digest: before.digest,
        lifecycle: "open".into(),
        author_node_id: None,
        author_login: None,
        metadata_digest: before.metadata_digest,
    };
    assert!(!reconcile::observation_unchanged(&prior, &after));
    http.abort();
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // Keep both repositories and their fault ordering in one scenario.
async fn permission_failure_in_one_repository_does_not_starve_other_mentions() {
    use axum::http::StatusCode;
    use axum::response::IntoResponse as _;
    let f = Fixture::new(false);
    let policy = SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 };
    let mut healthy_event = String::new();
    for index in 0..17 {
        let repository = if index < 16 { "owner/blocked" } else { "owner/healthy" };
        let id = format!("mention-{index}");
        let mut event = revision(&id, "@braid please continue");
        event.repository = repository.into();
        event.repository_node_id = repository.into();
        event.work_item_node_id = Some(repository.into());
        event.object_node_id = Some(id.clone());
        event.event_name = "issue_comment".into();
        event.action = Some("created".into());
        event.kind = EventKind::Wake;
        event.detail = Some("created");
        event.cross_surface_invalidation = false;
        event.mention_candidate = true;
        let result = f.store.ingest_event(event, policy).unwrap();
        if index == 16 {
            healthy_event = result.event_id.unwrap();
        }
    }
    let checked = Arc::new(tokio::sync::Notify::new());
    let notified = checked.clone();
    let router = Router::new()
        .route(
            "/repos/owner/blocked/collaborators/human/permission",
            get(|| async {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"message":"Resource not accessible by integration"})),
                )
                    .into_response()
            }),
        )
        .route(
            "/repos/owner/healthy/collaborators/human/permission",
            get(move || {
                let notified = notified.clone();
                async move {
                    notified.notify_one();
                    Json(json!({"permission":"admin","role_name":"admin"}))
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let http = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = octocrab::Octocrab::builder()
        .base_uri(format!("http://{address}"))
        .unwrap()
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .build()
        .unwrap();
    let clients = ["owner/blocked", "owner/healthy"]
        .into_iter()
        .map(|repository| {
            let identity = crate::github::GitHubIdentity {
                app_id: 1,
                app_slug: "braid".into(),
                installation_id: 1,
                repository: repository.into(),
                repository_node_id: repository.into(),
                actor_node_id: "app".into(),
                actor_login: "braid[bot]".into(),
                token_expires_at: String::new(),
                permissions: BTreeMap::new(),
            };
            (
                repository.into(),
                Arc::new(GitHubClient::publication_test_client(
                    client.clone(),
                    identity,
                    f.config.github().unwrap().clone(),
                )),
            )
        })
        .collect();
    let store = Arc::new(
        StoreActor::start(
            f.config.runtime.database().to_owned(),
            f.config.runtime.backups().to_owned(),
        )
        .unwrap(),
    );
    let (shutdown, receiver) = tokio::sync::watch::channel(false);
    let worker =
        tokio::spawn(crate::producer::mention_worker(store, Arc::new(clients), policy, receiver));
    tokio::time::timeout(Duration::from_secs(3), checked.notified())
        .await
        .expect("healthy repository must reach its permission check despite older denied mentions");
    shutdown.send(true).unwrap();
    worker.await.unwrap();
    let trusted: bool = f
        .db()
        .query_row("SELECT trusted_mention FROM events WHERE event_id=?1", [healthy_event], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(trusted);
    assert_eq!(f.store.mention_candidates("owner/blocked".into(), 16).unwrap().len(), 16);
    http.abort();
}
