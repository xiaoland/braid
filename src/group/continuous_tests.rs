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
use rusqlite::OptionalExtension as _;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Default)]
struct Captured {
    memory: Mutex<Vec<String>>,
    inputs: Mutex<Vec<String>>,
    resumed: Mutex<Vec<String>>,
    steer_error: Mutex<Option<SessionError>>,
    close_error: Mutex<Option<SessionError>>,
    close_attempts: Mutex<Vec<String>>,
    close_terminal: Mutex<Option<(crate::agent_session::TurnOutcome, Option<String>)>>,
    closed: tokio::sync::Notify,
    steered: tokio::sync::Notify,
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
        let mut memories = self.0.memory.lock().unwrap();
        memories.push(context);
        let id = if memories.len() == 1 {
            "fresh-provider".to_owned()
        } else {
            format!("fresh-provider-{}", memories.len())
        };
        Ok(CreatedSession {
            id: id.clone(),
            session: Arc::new(Session {
                id,
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
        self.0.resumed.lock().unwrap().push(id.to_owned());
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
        if let Some(error) = self.captured.close_error.lock().unwrap().clone() {
            return Err(error);
        }
        if let Some((outcome, error)) = self.captured.close_terminal.lock().unwrap().take() {
            let _ = self.events.send(SessionEvent::TurnTerminal {
                provider_turn_id: "after-revision".into(),
                outcome,
                error,
            });
        }
        Ok(())
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
        self.captured.steered.notify_one();
        Err(self.captured.steer_error.lock().unwrap().clone().unwrap_or_else(|| {
            SessionError::Failed("revision must wait for the safe boundary".into())
        }))
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
    assert!(
        matches!(error, crate::store::StoreError::Sqlite(ref sql) if sql.to_string().contains("unavailable publisher")),
        "{error}"
    );
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
        let spec =
            GroupSpec::new(GroupKind::Issue, &f.config, &f.store, "owner/repository").unwrap();
        f.store
            .ingest_event(revision(), SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 })
            .unwrap();
        let reset = f
            .store
            .begin_context_reset(None, "issue".into(), "issue-codex".into())
            .unwrap()
            .unwrap();
        assert_eq!(reset.active_turn_id, None);
        assert_eq!(
            f.store.runtime_status().unwrap().agent_groups[0].session_lifecycle.as_deref(),
            Some("reset_pending")
        );
        if confirm_stop {
            let driver = worker::GroupDriver {
                store: &f.store,
                config: &f.config,
                collaboration: &collaboration,
                spec: &spec,
                sessions: SessionManager::new(Arc::new(Factory(captured.clone()))),
            };
            driver
                .sessions
                .resume("provider".into(), spec.profile.clone(), "instructions".into())
                .await
                .unwrap();
            assert!(driver.quiesce_session("provider").await.unwrap());
            assert_eq!(*captured.close_attempts.lock().unwrap(), ["provider"]);
        }
        drop(f.store);
        let store = StoreActor::start(
            f.config.runtime.database().into(),
            f.config.runtime.backups().into(),
        )
        .unwrap();
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
            assert!(
                status.agent_groups[0]
                    .context_error
                    .as_deref()
                    .unwrap()
                    .contains("no stop evidence")
            );
            assert!(captured.memory.lock().unwrap().is_empty());
            assert!(driver.start_next_agent_turn().await.is_none());
        }
        http.abort();
    }
}

#[tokio::test]
async fn retired_reset_history_cannot_replace_an_execution_or_block_a_finite_run() {
    for blocked in [false, true] {
        let f = Fixture::new(true);
        let mut config = f.config.clone();
        config.scheduler.quiet_seconds = 0;
        let captured = Arc::new(Captured::default());
        let (github, http) = issue_context_server("Current contract").await;
        let collaboration = CollaborationBackend::GitHub(github);
        let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
        let driver = worker::GroupDriver {
            store: &f.store,
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
        let policy = SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 };
        f.store.ingest_event(revision(), policy).unwrap();
        let reset = f
            .store
            .begin_context_reset(None, "issue".into(), "issue-codex".into())
            .unwrap()
            .unwrap();
        if blocked {
            f.store
                .fail_context_reset(reset.reset_id.clone(), "old context is unavailable".into())
                .unwrap();
        }
        let before = f.store.execution_progress().unwrap();
        assert!(before.unresolved_reset);
        assert_eq!(before.blocked_reset.as_deref(), blocked.then_some(reset.reset_id.as_str()));
        let mut revoke = revision();
        revoke.delivery_guid = "assignment-removed-during-reset".into();
        revoke.identity = crate::store::ChangeIdentity::Occurrence;
        revoke.kind = EventKind::Unassign;
        revoke.action = Some("unassigned".into());
        revoke.requires_attention = false;
        revoke.cross_surface_invalidation = false;
        let event = f.store.ingest_event(revoke, policy).unwrap().event_id.unwrap();
        driver.settle_idle_unassignment(event).await.unwrap();
        let after = f.store.runtime_status().unwrap();
        assert_eq!(after.agent_groups[0].assignment_lifecycle, "retired");
        assert_eq!(
            after.context_resets[0].lifecycle,
            if blocked { "blocked" } else { "materializing" }
        );
        assert!(
            !driver.materialize_next_context_reset().await,
            "a retired generation cannot start replacement execution"
        );
        assert!(captured.memory.lock().unwrap().is_empty());
        let progress = f.store.execution_progress().unwrap();
        assert!(progress.blocked_reset.is_none(), "a historical reset cannot block a finite run");
        assert!(!progress.unresolved_reset, "retired reset history is no longer outstanding work");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builtin_turn_runs_without_writer_service_and_cannot_mint_after_reassignment() {
    builtin_running_unassignment(Some((crate::agent_session::TurnOutcome::Completed, None)), false)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn worker_preserves_a_failed_turn_during_unassignment_while_the_platform_fence_is_offline() {
    builtin_running_unassignment(
        Some((
            crate::agent_session::TurnOutcome::Failed,
            Some("provider failed while stopping".into()),
        )),
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restarted_worker_retires_a_stopped_unassignment_without_a_provider_terminal() {
    builtin_running_unassignment(None, true).await;
}

#[allow(clippy::too_many_lines)]
async fn builtin_running_unassignment(
    terminal: Option<(crate::agent_session::TurnOutcome, Option<String>)>,
    restart: bool,
) {
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
    let fence_offline = Arc::new(AtomicBool::new(false));
    let routes = crate::platform::router(&platform).unwrap().layer(axum::middleware::from_fn({
        let offline = Arc::clone(&offline);
        let fence_offline = Arc::clone(&fence_offline);
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let offline = Arc::clone(&offline);
            let fence_offline = Arc::clone(&fence_offline);
            async move {
                use axum::response::IntoResponse as _;
                if request.uri().path().starts_with("/api/platform/grants")
                    && offline.load(Ordering::Relaxed)
                {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                if request.uri().path() == "/api/platform/executions/fence"
                    && fence_offline.load(Ordering::Relaxed)
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
    config.profiles.iter_mut().find(|profile| profile.id == "issue-codex").unwrap().workspace =
        Some(source.clone());
    config.builtin = Some(crate::config::BuiltinConfig {
        repository: platform.repository.clone(),
        endpoint: format!("http://{address}").parse().unwrap(),
    });
    let (mut store, mut client) = open_worker_runtime(&config, "fixture").await;
    let actor = "issue-codex";
    let mut platform = PlatformObjects::open(&platform.database, &platform.repository).unwrap();
    let issue =
        platform.create_child_issue("Writer independence", "Read first", "human", None).unwrap();
    platform.assign(issue.number, Some(actor), "human").unwrap();
    let policy = SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 };
    let mut cursor = 0;
    for pending in platform.events(cursor, 100).unwrap() {
        store.ingest_builtin_event(pending.event, policy, pending.event_id).unwrap();
        cursor = pending.event_id;
    }
    let captured = Arc::new(Captured::default());
    let (shutdown, stop) = tokio::sync::watch::channel(false);
    let (reports, _receiver) = tokio::sync::mpsc::channel(8);
    let spawn = |store: &Arc<StoreActor>, client: &Arc<BuiltinClient>| {
        let spec = GroupSpec::new(GroupKind::Issue, &config, store, "test-space").unwrap();
        tokio::spawn(agent_group_worker_backend(
            Arc::clone(store),
            Arc::new(CollaborationBackend::Builtin(Arc::clone(client))),
            config.clone(),
            spec,
            Arc::new(Factory(Arc::clone(&captured))),
            reports.clone(),
            stop.clone(),
        ))
    };
    let mut worker = spawn(&store, &client);
    let limit = std::time::Duration::from_secs(5);
    tokio::time::timeout(limit, async {
        while !store
            .runtime_status()
            .unwrap()
            .agent_groups
            .iter()
            .any(|group| group.assignment_generation == 1 && group.assignment_lifecycle == "active")
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("assignment materializes before the first user input");
    platform.reply(issue.number, "Run this turn", "human", None).unwrap();
    for pending in platform.events(cursor, 100).unwrap() {
        store.ingest_builtin_event(pending.event, policy, pending.event_id).unwrap();
        cursor = pending.event_id;
    }
    store.advance_scheduler().unwrap();

    let (session_id, turn_id) = tokio::time::timeout(limit, async {
        loop {
            let running = f
                .db()
                .query_row(
                    "SELECT ps.provider_session_id,t.turn_id FROM turns t
                 JOIN provider_sessions ps ON ps.session_id=t.session_id
                 WHERE t.lifecycle='running' AND t.provider_turn_id='after-revision'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .unwrap();
            if let Some(running) = running {
                break running;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("writer service must not gate a real turn");
    assert_eq!(captured.inputs.lock().unwrap().len(), 1);
    let writer =
        BuiltinWriterTurn { session_id, turn_id, item_number: issue.number, actor: actor.into() };
    let offline_error = client.writer_grant(writer.clone()).await.unwrap_err();
    assert!(offline_error.to_string().contains("503"), "{offline_error:#}");

    // The old turn never minted a grant. Name equality must not revive it
    // after an unassign/reassign and before those events reach the Harness.
    platform.assign(issue.number, Some(actor), "human").unwrap();
    offline.store(false, Ordering::Relaxed);
    let error = client.writer_grant(writer.clone()).await.unwrap_err();
    assert!(error.to_string().contains("writer authority changed"), "{error:#}");
    offline.store(true, Ordering::Relaxed);
    captured.close_terminal.lock().unwrap().clone_from(&terminal);
    fence_offline.store(true, Ordering::Relaxed);
    for pending in platform.events(cursor, 100).unwrap() {
        store.ingest_builtin_event(pending.event, policy, pending.event_id).unwrap();
    }
    // The adapter returned a terminal during close. The worker must retain it
    // even across a failed independent fence, before creating another generation.
    let turn_state = || {
        f.db()
            .query_row(
                "SELECT lifecycle,error FROM turns WHERE turn_id=?1",
                [&writer.turn_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .unwrap()
    };
    let (outcome, reason) = if let Some((outcome, reason)) = terminal {
        tokio::time::timeout(limit, async {
            while turn_state().0 != outcome.lifecycle() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("observed provider terminal must be durable while the platform fence is offline");
        assert_eq!(turn_state().1, reason);
        (outcome, reason)
    } else {
        tokio::time::timeout(limit, async {
            while !f.db().query_row(
                "SELECT execution_stopped_at IS NOT NULL FROM provider_sessions WHERE provider_session_id=?1",
                [&writer.session_id], |row| row.get::<_, bool>(0),
            ).unwrap() {
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        }).await.expect("native stop must reach SQLite before a successful platform fence");
        assert_eq!(turn_state().0, "running");
        (
            crate::agent_session::TurnOutcome::Unknown,
            Some("unassignment did not observe the active turn terminal".to_owned()),
        )
    };
    assert_eq!(*captured.close_attempts.lock().unwrap(), ["fresh-provider"]);
    assert_eq!(store.runtime_status().unwrap().agent_groups[0].assignment_lifecycle, "active");
    if restart {
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        drop(client);
        drop(store);
        // A dead owner no longer renews its lease. Advance just that deadline.
        f.db().execute("UPDATE owner_leases SET expires_at='2000-01-01T00:00:00Z' WHERE owner_id='fixture'", []).unwrap();
        (store, client) = open_worker_runtime(&config, "next-runtime").await;
        fence_offline.store(false, Ordering::Relaxed);
        client.fence_all_executions().await.unwrap();
        worker = spawn(&store, &client);
    } else {
        fence_offline.store(false, Ordering::Relaxed);
    }
    let groups = tokio::time::timeout(limit, async {
        loop {
            let groups = store.runtime_status().unwrap().agent_groups;
            assert!(
                groups
                    .iter()
                    .all(|g| g.assignment_generation != 1 || g.assignment_lifecycle != "blocked"),
                "a stopped unassignment must retire instead of becoming a blocked lost turn"
            );
            assert!(
                captured.resumed.lock().unwrap().is_empty(),
                "a stopped unassignment must never resume its old native session"
            );
            if groups.iter().any(|group| {
                group.assignment_generation == 2 && group.assignment_lifecycle == "active"
            }) {
                break groups;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("retire the old generation and materialize its immutable reassignment");
    let retired = groups.iter().find(|group| group.assignment_generation == 1).unwrap();
    let current = groups.iter().find(|group| group.assignment_generation == 2).unwrap();
    assert_eq!(
        retired.assignment_lifecycle, "retired",
        "the stopped old generation must retire after recovery"
    );
    assert!(
        captured.resumed.lock().unwrap().is_empty(),
        "a stopped unassignment must never resume its old native session"
    );
    assert_eq!(current.provider_session_id.as_deref(), Some("fresh-provider-2"));
    if let Some(reason) = &reason {
        let body: String = f.db().query_row("SELECT body FROM operational_status_facts WHERE assignment_id=(SELECT assignment_id FROM assignments WHERE generation=1)", [], |row| row.get(0)).expect("the failed or unknown old turn must have a configured Operational Status");
        assert!(body.contains(outcome.lifecycle()));
        assert!(body.contains(reason));
    }
    assert_eq!(turn_state(), (outcome.lifecycle().to_owned(), reason));
    assert_eq!(*captured.close_attempts.lock().unwrap(), ["fresh-provider"]);
    assert_eq!(captured.memory.lock().unwrap().len(), 2);
    client.fence_all_executions().await.unwrap();
    let fenced = client.writer_grant(writer).await.unwrap_err();
    assert!(fenced.to_string().contains("no current writer authority"));
    shutdown.send(true).unwrap();
    worker.await.unwrap().unwrap();
    http.abort();
}

async fn open_worker_runtime(
    config: &crate::config::Config,
    owner: &str,
) -> (Arc<StoreActor>, Arc<crate::builtin::BuiltinClient>) {
    let store = Arc::new(
        StoreActor::start(config.runtime.database().into(), config.runtime.backups().into())
            .unwrap(),
    );
    let lease = store.acquire_runtime_lease("runtime".into(), owner.into(), 60).unwrap();
    let client = crate::builtin::BuiltinClient::connect(config, Arc::clone(&store))
        .await
        .unwrap()
        .with_runtime(lease)
        .unwrap();
    (store, client)
}

#[tokio::test]
async fn worker_stops_a_running_unassigned_issue_only_after_its_quiet_window() {
    use sha2::{Digest as _, Sha256};
    use tokio::time::{Duration, timeout};

    let f = Fixture::new(true);
    let config = f.config.clone();
    let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
    std::fs::create_dir_all(spec.profile.workspace()).unwrap();
    let instructions = provider::issue_system_prompt(&config, &spec.profile, 1);
    f.db()
        .execute(
            "UPDATE provider_sessions SET instruction_revision=?1",
            [hex::encode(Sha256::digest(instructions.as_bytes()))],
        )
        .unwrap();
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("Current instructions").await;
    let store = Arc::new(
        StoreActor::start(config.runtime.database().into(), config.runtime.backups().into())
            .unwrap(),
    );
    let policy = SchedulerPolicy { quiet_seconds: 0, event_threshold: 1 };
    let mut wake = revision();
    wake.kind = EventKind::Wake;
    wake.cross_surface_invalidation = false;
    store.ingest_event(wake, policy).unwrap();
    let (shutdown, stop) = tokio::sync::watch::channel(false);
    let (reports, _receiver) = tokio::sync::mpsc::channel(8);
    let worker = tokio::spawn(agent_group_worker(
        Arc::clone(&store),
        github,
        config,
        spec,
        Arc::new(Factory(Arc::clone(&captured))),
        reports,
        stop,
    ));
    let limit = Duration::from_secs(5);
    timeout(limit, async {
        while !f.db().query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE provider_turn_id='after-revision' AND lifecycle='running')", [], |row| row.get::<_, bool>(0)).unwrap() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("the provider must start a real turn before unassignment");
    let mut unassign = revision();
    unassign.delivery_guid = "assignment-removed".into();
    unassign.identity = crate::store::ChangeIdentity::Occurrence;
    unassign.kind = EventKind::Unassign;
    unassign.action = Some("unassigned".into());
    unassign.cross_surface_invalidation = false;
    unassign.requires_attention = false;
    let event = store.ingest_event(unassign, policy).unwrap().event_id.unwrap();

    // A second provider input acknowledges a worker tick inside the window.
    let mut urgent = revision();
    urgent.delivery_guid = "urgent-while-assigned".into();
    urgent.object_version = Some("v3".into());
    urgent.kind = EventKind::Wake;
    urgent.cross_surface_invalidation = false;
    urgent.mention_candidate = true;
    let mention = store.ingest_event(urgent, policy).unwrap().event_id.unwrap();
    store.resolve_mention(mention, true, policy).unwrap();
    timeout(limit, captured.steered.notified()).await.expect("turn stays active during debounce");
    assert!(captured.close_attempts.lock().unwrap().is_empty());

    *captured.close_error.lock().unwrap() =
        Some(SessionError::Unknown("native task still alive".into()));
    f.db()
        .execute("UPDATE events SET observed_at='2000-01-01T00:00:00Z' WHERE event_id=?1", [&event])
        .unwrap();
    timeout(limit, captured.closed.notified())
        .await
        .expect("settled unassignment must stop a running native turn");
    assert_eq!(store.runtime_status().unwrap().agent_groups[0].assignment_lifecycle, "active");
    *captured.close_error.lock().unwrap() = None;
    timeout(limit, async {
        while store.runtime_status().unwrap().agent_groups[0].assignment_lifecycle != "retired" {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("confirmed native stop must retire the generation");
    let terminal: String = f
        .db()
        .query_row(
            "SELECT lifecycle FROM turns WHERE provider_turn_id='after-revision'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(terminal, "unknown", "closing the session alone cannot identify a turn outcome");
    assert_eq!(captured.inputs.lock().unwrap().len(), 1);
    shutdown.send(true).unwrap();
    worker.await.unwrap().unwrap();
    let status = f
        .db()
        .query_row(
            "SELECT body FROM operational_status_facts WHERE assignment_id='assignment'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .unwrap();
    assert!(
        status.as_deref().is_some_and(|body| body.contains("执行状态：unknown")
            && body.contains("unassignment did not observe the active turn terminal")),
        "an unobserved turn terminal must publish the configured operational status: {status:?}"
    );
    http.abort();
}
