use super::*;
use crate::{
    agent_session::{
        AgentSession, CreatedSession, SendResult, SessionError, SessionEvent, SessionFactory,
    },
    collaboration::CollaborationBackend,
    config::Profile,
    store::{EventKind, IngressEvent, SchedulerPolicy, StoreActor},
    writer::tests::{Fixture, issue_context_server},
};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Default)]
struct Captured {
    memory: Mutex<Vec<String>>,
    inputs: Mutex<Vec<String>>,
    steer_error: Mutex<Option<SessionError>>,
    close_error: Mutex<Option<SessionError>>,
    close_attempts: Mutex<Vec<String>>,
    closed: tokio::sync::Notify,
}
struct Factory(Arc<Captured>);
struct Session {
    id: String,
    captured: Arc<Captured>,
    events: broadcast::Sender<SessionEvent>,
}
#[async_trait::async_trait]
impl SessionFactory for Factory {
    async fn check(&self) -> Result<(), SessionError> {
        Ok(())
    }
    async fn start(
        &self,
        _profile: Profile,
        _instructions: String,
        context: String,
    ) -> Result<CreatedSession, SessionError> {
        self.0.memory.lock().unwrap().push(context);
        Ok(CreatedSession {
            id: "fresh-provider".into(),
            session: Arc::new(Session {
                id: "fresh-provider".into(),
                captured: self.0.clone(),
                events: broadcast::channel(8).0,
            }),
        })
    }
    async fn resume(
        &self,
        id: &str,
        _profile: Profile,
        _instructions: String,
    ) -> Result<CreatedSession, SessionError> {
        assert_eq!(id, "provider", "only the existing session is restored");
        Ok(CreatedSession {
            id: id.into(),
            session: Arc::new(Session {
                id: id.into(),
                captured: self.0.clone(),
                events: broadcast::channel(8).0,
            }),
        })
    }
}
#[async_trait::async_trait]
impl AgentSession for Session {
    fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }
    fn is_unavailable(&self) -> bool {
        false
    }
    async fn close(&self) -> Result<(), SessionError> {
        self.captured.closed.notify_one();
        self.captured.close_attempts.lock().unwrap().push(self.id.clone());
        self.captured.close_error.lock().unwrap().clone().map_or(Ok(()), Err)
    }
    async fn send_user_msg(&self, msg: String, steering: bool) -> Result<SendResult, SessionError> {
        assert!(!steering);
        self.captured.inputs.lock().unwrap().push(msg);
        self.events
            .send(SessionEvent::TurnStarted { provider_turn_id: "after-revision".into() })
            .unwrap();
        Ok(SendResult::Started)
    }
    async fn steer_user_msg(
        &self,
        _msg: String,
        _expected: &str,
    ) -> Result<SendResult, SessionError> {
        Err(self.captured.steer_error.lock().unwrap().clone().unwrap_or_else(|| {
            SessionError::Failed("revision must wait for the safe boundary".into())
        }))
    }
    async fn interrupt(&self) -> Result<(), SessionError> {
        Ok(())
    }
}
fn revision() -> IngressEvent {
    IngressEvent {
        identity: crate::store::ChangeIdentity::ObjectVersion,
        delivery_guid: "human-body-revision".into(),
        event_name: "issues".into(),
        action: Some("edited".into()),
        repository_node_id: "repo".into(),
        repository: "owner/repository".into(),
        work_item_node_id: Some("item".into()),
        work_item_kind: Some("issue"),
        work_item_number: Some(1),
        work_item_state: Some("open".into()),
        object_node_id: Some("item".into()),
        object_version: Some("v2".into()),
        object_digest: Some("b".repeat(64)),
        visible_body: Some("The new contract".into()),
        actor_node_id: Some("human".into()),
        actor_login: Some("human".into()),
        kind: EventKind::Invalidate,
        detail: Some("edited"),
        cross_surface_invalidation: true,
        requires_attention: true,
        origin: "external",
        reference: "Human revised the Issue description".into(),
        mention_candidate: false,
        reaction_target: None,
        known: true,
        raw_payload: vec![],
    }
}

#[tokio::test]
async fn soft_pressure_reset_replaces_memory_and_delivers_input_with_publication_unavailable() {
    let f = Fixture::new(true);
    let mut config = f.config.clone();
    config
        .profiles
        .iter_mut()
        .find(|profile| profile.id == "issue-codex")
        .unwrap()
        .github_context_soft_ratio = 0.000_01;
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("The new contract: authorized Twitter User").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
    f.db()
        .execute(
            "UPDATE agent_instances SET profile_revision=?1",
            [i64::try_from(spec.profile_record.revision).unwrap()],
        )
        .unwrap();
    f.db()
        .execute_batch(
            "CREATE TRIGGER outbox_offline BEFORE INSERT ON github_write_outbox
         BEGIN SELECT RAISE(ABORT, 'unavailable publisher'); END;",
        )
        .unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
    };
    driver
        .sessions
        .resume("provider".into(), spec.profile.clone(), "old instructions".into())
        .await
        .unwrap();
    let policy = SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 };
    f.store.ingest_event(revision(), policy).unwrap();
    f.store.ingest_event(revision(), policy).unwrap();
    assert!(driver.materialize_next_context_reset().await);
    assert!(!driver.materialize_next_context_reset().await);
    assert_eq!(*captured.close_attempts.lock().unwrap(), ["provider"]);
    let memory = captured.memory.lock().unwrap().clone();
    assert_eq!(memory.len(), 1);
    assert!(memory[0].contains("authorized Twitter User"));
    assert_eq!(f.store.runtime_status().unwrap().agent_groups[0].context_pressure, "soft");
    assert_eq!(
        f.db()
            .query_row("SELECT count(*) FROM operational_status_facts", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    // Resetting an idle session is not an acknowledgement of the work input.
    assert_eq!(f.store.runtime_status().unwrap().batches.len(), 1);
    assert!(driver.start_next_agent_turn().await.is_none(), "ordinary revision must keep debounce");
    f.db().execute("UPDATE wake_batches SET quiet_deadline='2000-01-01T00:00:00Z' WHERE lifecycle='pending'",[]).unwrap();
    f.store.advance_scheduler().unwrap();
    let active = driver
        .start_next_agent_turn()
        .await
        .expect("revision starts native work without another Human comment");
    assert_eq!(active.provider_turn_id, "after-revision");
    assert_eq!(active.claim.trigger_kind, "wake_batch");
    assert_eq!(active.claim.references, ["Human revised the Issue description"]);
    assert!(!active.claim.trusted_mention);
    assert_eq!(captured.inputs.lock().unwrap().len(), 1);
    assert!(driver.start_next_agent_turn().await.is_none());
    assert_eq!(
        f.db()
            .query_row("SELECT count(*) FROM context_resets WHERE lifecycle='applied'", [], |r| r
                .get::<_, i64>(
                0
            ))
            .unwrap(),
        1
    );
    let publisher =
        f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 60).unwrap();
    let error = f.store.claim_github_write(publisher.clone()).unwrap_err();
    assert!(matches!(error, crate::store::StoreError::Sqlite(ref sql) if sql.to_string().contains("unavailable publisher")), "{error}");
    f.db().execute_batch("DROP TRIGGER outbox_offline").unwrap();
    let status = f.store.claim_github_write(publisher.clone()).unwrap().unwrap();
    assert!(status.content.contains("complete Context without truncation"));
    f.store.release_runtime_lease(publisher).unwrap();
    http.abort();
}

#[tokio::test]
async fn unavailable_complete_input_stays_pending_with_a_visible_reason() {
    let f = Fixture::new(true);
    let mut config = f.config.clone();
    config.profiles.iter_mut().find(|p| p.id == "issue-codex").unwrap().github_context_hard_bytes =
        1;
    let captured = Arc::new(Captured::default());
    let (github, http) =
        issue_context_server("A complete contract that exceeds the test budget").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
    let sessions = SessionManager::new(Arc::new(Factory(captured.clone())));
    sessions
        .start(spec.profile.clone(), "instructions".into(), "existing memory".into())
        .await
        .unwrap();
    f.db()
        .execute("UPDATE provider_sessions SET provider_session_id='fresh-provider'", [])
        .unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions,
    };
    let mut input = revision();
    input.kind = EventKind::Wake;
    input.cross_surface_invalidation = false;
    f.store.ingest_event(input, SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 }).unwrap();
    assert!(driver.start_next_agent_turn().await.is_none());
    let status = f.store.runtime_status().unwrap();
    assert_eq!(status.batches.len(), 1);
    assert!(status.batches[0].input_error.as_deref().unwrap().contains("exceeds hard limit"));
    assert!(captured.inputs.lock().unwrap().is_empty());
    http.abort();
}

#[tokio::test]
async fn reopened_generation_cannot_silently_adopt_changed_profile_materials() {
    let f = Fixture::new(true);
    f.db()
        .execute_batch(
            "UPDATE assignments SET lifecycle='sleeping';
             UPDATE agent_instances SET lifecycle='sleeping';
             UPDATE provider_sessions SET lifecycle='sleeping';
             UPDATE worktrees SET lifecycle='sleeping';",
        )
        .unwrap();
    let mut config = f.config.clone();
    config.profiles.iter_mut().find(|p| p.id == "issue-codex").unwrap().model =
        Some("changed-model".into());
    let mut event = revision();
    event.kind = EventKind::Lifecycle;
    event.action = Some("reopened".into());
    event.detail = Some("reopened");
    event.cross_surface_invalidation = false;
    f.store.ingest_event(event, SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 }).unwrap();
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("Current context").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
    };
    let candidate = f
        .store
        .work_item_lifecycle_candidates("owner/repository".into(), "issue".into(), 1)
        .unwrap()
        .pop()
        .unwrap();
    let error = driver.reactivate_work_item_agent(candidate).await.unwrap_err();
    assert!(error.to_string().contains("does not match active Profile"));
    assert!(captured.memory.lock().unwrap().is_empty(), "must not create a physical session");
    assert!(captured.inputs.lock().unwrap().is_empty());
    assert_eq!(f.store.runtime_status().unwrap().agent_groups[0].assignment_lifecycle, "blocked");
    http.abort();
}

#[tokio::test]
async fn pending_reset_cannot_silently_adopt_changed_profile_materials() {
    let f = Fixture::new(true);
    f.store
        .ingest_event(revision(), SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 })
        .unwrap();
    // The reset was persisted under the old Profile; its old handle can still stop.
    f.store.begin_context_reset(None, "issue".into(), "issue-codex".into()).unwrap().unwrap();
    let mut config = f.config.clone();
    config.profiles.iter_mut().find(|p| p.id == "issue-codex").unwrap().model =
        Some("changed-model".into());
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("Current context").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
    let sessions = SessionManager::new(Arc::new(Factory(captured.clone())));
    sessions.resume("provider".into(), spec.profile.clone(), "instructions".into()).await.unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions,
    };
    assert!(driver.materialize_next_context_reset().await);
    assert!(captured.memory.lock().unwrap().is_empty(), "must not create a physical session");
    assert!(captured.inputs.lock().unwrap().is_empty());
    let status = f.store.runtime_status().unwrap();
    assert_eq!(status.agent_groups[0].assignment_lifecycle, "active");
    assert_eq!(status.agent_groups[0].session_lifecycle.as_deref(), Some("blocked"));
    assert!(
        status.agent_groups[0]
            .context_error
            .as_deref()
            .unwrap()
            .contains("does not match active Profile")
    );
    assert_eq!(status.context_resets[0].lifecycle, "blocked");
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );
    http.abort();
}

#[tokio::test]
async fn unknown_replay_cannot_replace_a_session_without_confirmed_stop() {
    let f = Fixture::new(true);
    let mut event = revision();
    event.kind = EventKind::Wake;
    event.cross_surface_invalidation = false;
    f.store.ingest_event(event, SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 }).unwrap();
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
        .unwrap()
        .unwrap();
    f.store.mark_turn_started(claim.turn_id.clone(), "lost-turn".into()).unwrap();
    f.store.mark_turn_terminal(claim.turn_id, "unknown".into()).unwrap();
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("Current context").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &f.config, &f.store, "owner/repository").unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &f.config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
    };
    assert!(driver.materialize_next_context_reset().await);
    assert!(captured.memory.lock().unwrap().is_empty());
    assert!(captured.inputs.lock().unwrap().is_empty());
    let status = f.store.runtime_status().unwrap();
    assert_eq!(status.agent_groups[0].assignment_lifecycle, "active");
    // A failed replacement must preserve the old execution's uncertainty.
    assert_eq!(status.agent_groups[0].session_lifecycle.as_deref(), Some("unknown"));
    assert_eq!(status.context_resets[0].lifecycle, "blocked");
    assert!(status.agent_groups[0].context_error.as_deref().unwrap().contains("no stop evidence"));
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );
    assert_eq!(status.batches.len(), 1, "the replay obligation remains visible");
    assert_eq!(status.batches[0].event_count, 1);
    http.abort();
}

#[tokio::test]
async fn persisted_idle_reset_uses_only_confirmed_native_stop_after_restart() {
    for confirm_stop in [false, true] {
        let f = Fixture::new(true);
        let captured = Arc::new(Captured::default());
        let (github, http) = issue_context_server("Persisted revised contract").await;
        let collaboration = CollaborationBackend::GitHub(github);
        let spec = GroupSpec::new(GroupKind::Issue, &f.config, &f.store, "owner/repository").unwrap();
        f.store.ingest_event(revision(), SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 }).unwrap();
        let reset = f.store.begin_context_reset(None, "issue".into(), "issue-codex".into()).unwrap().unwrap();
        assert_eq!(reset.active_turn_id, None);
        assert_eq!(f.store.runtime_status().unwrap().agent_groups[0].session_lifecycle.as_deref(), Some("reset_pending"));
        if confirm_stop {
            let driver = worker::GroupDriver {
                store: &f.store,
                config: &f.config,
                collaboration: &collaboration,
                spec: &spec,
                sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
            };
            driver.sessions.resume("provider".into(), spec.profile.clone(), "instructions".into()).await.unwrap();
            assert!(driver.quiesce_session("provider").await.unwrap());
            assert_eq!(*captured.close_attempts.lock().unwrap(), ["provider"]);
        }
        drop(f.store);
        let store = StoreActor::start(f.config.runtime.database().into(), f.config.runtime.backups().into()).unwrap();
        let driver = worker::GroupDriver {
            store: &store,
            config: &f.config,
            collaboration: &collaboration,
            spec: &spec,
            sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
        };
        assert!(driver.materialize_next_context_reset().await);
        let status = store.runtime_status().unwrap();
        assert_eq!(status.batches.len(), 1, "the revised contract still requires attention");
        if confirm_stop {
            assert_eq!(status.context_resets[0].lifecycle, "applied");
            assert!(!driver.materialize_next_context_reset().await);
            let memory = captured.memory.lock().unwrap().clone();
            assert_eq!(memory.len(), 1);
            assert!(memory[0].contains("Persisted revised contract"));
            let running = driver.start_next_agent_turn().await.unwrap();
            assert_eq!(running.claim.references, ["Human revised the Issue description"]);
            assert_eq!(captured.inputs.lock().unwrap().len(), 1);
        } else {
            assert_eq!(status.context_resets[0].lifecycle, "blocked");
            assert!(status.agent_groups[0].context_error.as_deref().unwrap().contains("no stop evidence"));
            assert!(captured.memory.lock().unwrap().is_empty());
            assert!(driver.start_next_agent_turn().await.is_none());
        }
        http.abort();
    }
}

#[tokio::test]
async fn failed_idle_stop_keeps_the_handle_and_blocks_replacement() {
    let f = Fixture::new(true);
    let captured = Arc::new(Captured::default());
    *captured.close_error.lock().unwrap() =
        Some(SessionError::Unknown("owned task still alive".into()));
    let (github, http) = issue_context_server("Current context").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &f.config, &f.store, "owner/repository").unwrap();
    let sessions = SessionManager::new(Arc::new(Factory(captured.clone())));
    sessions.resume("provider".into(), spec.profile.clone(), "instructions".into()).await.unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &f.config,
        collaboration: &collaboration,
        spec: &spec,
        sessions,
    };
    f.store
        .ingest_event(revision(), SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 })
        .unwrap();
    assert!(driver.materialize_next_context_reset().await);
    assert_eq!(*captured.close_attempts.lock().unwrap(), ["provider"]);
    assert!(captured.memory.lock().unwrap().is_empty());
    assert!(captured.inputs.lock().unwrap().is_empty());
    let status = f.store.runtime_status().unwrap();
    assert_eq!(status.context_resets[0].lifecycle, "blocked");
    assert!(
        status.agent_groups[0].context_error.as_deref().unwrap().contains("owned task still alive")
    );
    assert_eq!(status.batches.len(), 1);
    *captured.close_error.lock().unwrap() = None;
    assert!(driver.quiesce_session("provider").await.unwrap());
    assert_eq!(*captured.close_attempts.lock().unwrap(), ["provider", "provider"]);
    http.abort();
}

#[tokio::test]
async fn rejected_steer_stops_retrying_while_unknown_steer_retains_its_input() {
    for unknown in [false, true] {
        let f = Fixture::new(true);
        let captured = Arc::new(Captured::default());
        if unknown {
            *captured.steer_error.lock().unwrap() =
                Some(SessionError::Unknown("lost reply".into()));
        }
        let (github, http) = issue_context_server("Current context").await;
        let collaboration = Arc::new(CollaborationBackend::GitHub(github));
        let spec =
            GroupSpec::new(GroupKind::Issue, &f.config, &f.store, "owner/repository").unwrap();
        let sessions = SessionManager::new(Arc::new(Factory(captured.clone())));
        sessions.start(spec.profile.clone(), "instructions".into(), "memory".into()).await.unwrap();
        f.db()
            .execute("UPDATE provider_sessions SET provider_session_id='fresh-provider'", [])
            .unwrap();
        let driver = worker::GroupDriver {
            store: &f.store,
            config: &f.config,
            collaboration: &collaboration,
            spec: &spec,
            sessions,
        };
        let policy = SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 };
        let mut initial = revision();
        initial.kind = EventKind::Wake;
        initial.cross_surface_invalidation = false;
        f.store.ingest_event(initial, policy).unwrap();
        let active = driver.start_next_agent_turn().await.unwrap();
        let mut urgent = revision();
        urgent.delivery_guid = "urgent-new-comment".into();
        urgent.object_version = Some("v3".into());
        urgent.kind = EventKind::Wake;
        urgent.cross_surface_invalidation = false;
        urgent.mention_candidate = true;
        urgent.reference = "New trusted mention".into();
        let admitted = f.store.ingest_event(urgent, policy).unwrap();
        f.store.resolve_mention(admitted.event_id.unwrap(), true, policy).unwrap();
        let fence = driver.forward_urgent_steer(&active).await;
        if unknown {
            assert!(fence.unwrap().contains("reception is unknown"));
            f.store.mark_turn_terminal(active.claim.turn_id, "unknown".into()).unwrap();
            let status = f.store.runtime_status().unwrap();
            assert_eq!(status.batches.len(), 1);
            assert_eq!(
                status.batches[0].event_count, 2,
                "original and uncertain steer both survive"
            );
        } else {
            assert!(fence.is_none());
            assert!(f.store.claim_urgent_steer(active.claim.turn_id).unwrap().is_none());
            assert!(f.store.runtime_status().unwrap().batches.is_empty());
        }
        http.abort();
    }
}

#[tokio::test]
async fn stopping_native_execution_does_not_wait_for_execution_fence() {
    use axum::{
        Json, Router,
        http::StatusCode,
        routing::{get, post},
    };
    use tokio::sync::Notify;

    let f = Fixture::new(false);
    let store = Arc::new(
        StoreActor::start(f.config.runtime.database().into(), f.config.runtime.backups().into())
            .unwrap(),
    );
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let routes = Router::new()
        .route("/api/platform/repository", get(|| async {
            Json(serde_json::json!({"repository":"test-space","spaceId":"space","executionFences":true,"nodeId":"local:test-space","defaultBranch":"main"}))
        }))
        .route("/api/platform/executions/fence", post({
            let (entered, release) = (entered.clone(), release.clone());
            move || {
                let (entered, release) = (entered.clone(), release.clone());
                async move {
                    entered.notify_one();
                    release.notified().await;
                    (StatusCode::SERVICE_UNAVAILABLE, "fence service offline")
                }
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let http = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    let mut config = f.config.clone();
    config.github = None;
    config.builtin = Some(crate::config::BuiltinConfig {
        repository: "test-space".into(),
        endpoint: format!("http://{address}").parse().unwrap(),
    });
    let lease = store.acquire_runtime_lease("runtime".into(), "fixture".into(), 60).unwrap();
    let collaboration = CollaborationBackend::Builtin(
        crate::builtin::BuiltinClient::connect(&config, Arc::clone(&store))
            .await
            .unwrap()
            .with_runtime(lease)
            .unwrap(),
    );
    let spec = GroupSpec::new(GroupKind::Issue, &config, &store, "test-space").unwrap();
    f.db().execute_batch(
        "INSERT INTO repositories VALUES('local:test-space','test-space',1,'now');
         INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
           VALUES('fixture-item','local:test-space','issue',1,'open','now');
         INSERT INTO assignments VALUES('fixture-assignment','fixture-item',1,'active','now',NULL);"
    ).unwrap();
    f.db().execute(
        "INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle)
         SELECT 'fixture-agent','fixture-assignment',profile_id,revision,'issue','idle'
         FROM profiles WHERE profile_id=?1",
        [&spec.profile.id],
    ).unwrap();
    f.db().execute(
        "INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,
          context_revision,instruction_revision,lifecycle,started_at,builtin_execution_id,builtin_execution_owner)
         VALUES('fixture-session','fixture-agent','codex','provider','memory','instructions','idle','now',?1,'fixture')",
        [uuid::Uuid::now_v7().to_string()],
    ).unwrap();
    let captured = Arc::new(Captured::default());
    let driver = worker::GroupDriver {
        store: &store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
    };
    driver
        .sessions
        .resume("provider".into(), spec.profile.clone(), "instructions".into())
        .await
        .unwrap();

    let (stop, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(driver.quiesce_session("provider"), async {
            entered.notified().await;
            captured.closed.notified().await;
            assert!(driver.sessions.confirmed_stopped("provider").await);
            release.notify_one();
        })
    })
    .await
    .expect("native close must finish while the fence response is withheld");
    assert!(stop.unwrap_err().to_string().contains("execution fence unconfirmed"));
    http.abort();
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn builtin_turn_runs_without_writer_service_and_cannot_mint_after_reassignment() {
    use crate::{
        builtin::BuiltinClient,
        platform::{PlatformConfig, objects::PlatformObjects},
        store::BuiltinWriterTurn,
    };
    use axum::http::StatusCode;
    use std::sync::atomic::{AtomicBool, Ordering};

    let f = Fixture::new(false);
    let root = f.config.runtime.database().parent().unwrap();
    let source = root.join("source");
    let git = git2::Repository::init_opts(
        &source,
        git2::RepositoryInitOptions::new().initial_head("main"),
    )
    .unwrap();
    let signature = git2::Signature::now("Braid Test", "test@example.invalid").unwrap();
    let tree = git.find_tree(git.treebuilder(None).unwrap().write().unwrap()).unwrap();
    git.commit(Some("HEAD"), &signature, &signature, "init", &tree, &[]).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let platform = PlatformConfig {
        schema_version: 1,
        repository: "test-space".into(),
        source: source.canonicalize().unwrap(),
        default_branch: "main".into(),
        database: root.join("platform.sqlite3"),
        backups: root.join("platform-backups"),
        listen: address.to_string(),
    };
    let offline = Arc::new(AtomicBool::new(true));
    let routes = crate::platform::router(&platform).unwrap().layer(axum::middleware::from_fn({
        let offline = Arc::clone(&offline);
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let offline = Arc::clone(&offline);
            async move {
                use axum::response::IntoResponse as _;
                if request.uri().path().starts_with("/api/platform/grants")
                    && offline.load(Ordering::Relaxed)
                {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                next.run(request).await
            }
        }
    }));
    let http = tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });

    let mut config = f.config.clone();
    config.github = None;
    config.builtin = Some(crate::config::BuiltinConfig {
        repository: platform.repository.clone(),
        endpoint: format!("http://{address}").parse().unwrap(),
    });
    let store = Arc::new(
        StoreActor::start(config.runtime.database().into(), config.runtime.backups().into())
            .unwrap(),
    );
    let lease = store.acquire_runtime_lease("runtime".into(), "fixture".into(), 60).unwrap();
    let client = BuiltinClient::connect(&config, Arc::clone(&store))
        .await
        .unwrap()
        .with_runtime(lease)
        .unwrap();
    let spec = GroupSpec::new(GroupKind::Issue, &config, &store, "test-space").unwrap();
    let mut platform = PlatformObjects::open(&platform.database, &platform.repository).unwrap();
    let issue =
        platform.create_child_issue("Writer independence", "Read first", "human", None).unwrap();
    platform.assign(issue.number, Some(&spec.profile.id), "human").unwrap();
    let policy = SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 };
    let mut cursor = 0;
    for pending in platform.events(cursor, 100).unwrap() {
        store.ingest_builtin_event(pending.event, policy, pending.event_id).unwrap();
        cursor = pending.event_id;
    }
    let assignment =
        store.assignment_candidates("test-space".into(), "issue".into(), 1).unwrap().pop().unwrap();
    let materialization = store
        .begin_agent_assignment(
            assignment.event_id,
            spec.profile_record.clone(),
            Some("old memory".into()),
            false,
        )
        .unwrap()
        .unwrap();
    store
        .complete_agent_assignment(
            materialization,
            "provider".into(),
            "old memory".into(),
            "instructions".into(),
        )
        .unwrap();
    platform.reply(issue.number, "Run this turn", "human", None).unwrap();
    for pending in platform.events(cursor, 100).unwrap() {
        store.ingest_builtin_event(pending.event, policy, pending.event_id).unwrap();
    }
    store.advance_scheduler().unwrap();

    let captured = Arc::new(Captured::default());
    let collaboration = CollaborationBackend::Builtin(Arc::clone(&client));
    let driver = worker::GroupDriver {
        store: &store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(Arc::clone(&captured)))),
    };
    driver
        .sessions
        .resume("provider".into(), spec.profile.clone(), "instructions".into())
        .await
        .unwrap();
    let running = driver.start_next_agent_turn().await.unwrap_or_else(|| {
        panic!("writer service must not gate a turn: {:?}", store.runtime_status().unwrap().batches)
    });
    assert_eq!(captured.inputs.lock().unwrap().len(), 1);
    let writer = BuiltinWriterTurn {
        session_id: running.claim.provider_session_id,
        turn_id: running.claim.turn_id,
        item_number: issue.number,
        actor: spec.profile.id.clone(),
    };
    let offline_error = client.writer_grant(writer.clone()).await.unwrap_err();
    assert!(offline_error.to_string().contains("503"));

    // The old turn never minted a grant. Name equality must not revive it
    // after an unassign/reassign and before those events reach the Harness.
    platform.assign(issue.number, Some(&spec.profile.id), "human").unwrap();
    offline.store(false, Ordering::Relaxed);
    let error = client.writer_grant(writer.clone()).await.unwrap_err();
    assert!(error.to_string().contains("writer authority changed"), "{error:#}");
    offline.store(true, Ordering::Relaxed);
    driver.require_quiesced(&writer.session_id, false).await.unwrap();
    client.fence_all_executions().await.unwrap();
    let fenced = client.writer_grant(writer).await.unwrap_err();
    assert!(fenced.to_string().contains("no current writer authority"));
    http.abort();
}
