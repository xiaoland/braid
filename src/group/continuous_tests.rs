use super::*;
use crate::{
    agent_session::{
        AgentSession, CreatedSession, SendResult, SessionError, SessionEvent, SessionFactory,
    },
    collaboration::CollaborationBackend,
    config::Profile,
    store::{EventKind, IngressEvent, SchedulerPolicy},
    writer::tests::{Fixture, issue_context_server},
};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

#[derive(Default)]
struct Captured {
    memory: Mutex<Vec<String>>,
    inputs: Mutex<Vec<String>>,
    steer_error: Mutex<Option<SessionError>>,
}
struct Factory(Arc<Captured>);
struct Session {
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
                captured: self.0.clone(),
                events: broadcast::channel(8).0,
            }),
        })
    }
    async fn resume(
        &self,
        _id: &str,
        _profile: Profile,
        _instructions: String,
    ) -> Result<CreatedSession, SessionError> {
        Err(SessionError::Failed("unexpected resume".into()))
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
async fn idle_revision_replaces_memory_then_starts_once_without_a_second_mention() {
    let f = Fixture::new(true);
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("The new contract: authorized Twitter User").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &f.config, &f.store, "owner/repository").unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &f.config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(captured.clone())), collaboration.clone()),
    };
    let policy = SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 };
    f.store.ingest_event(revision(), policy).unwrap();
    f.store.ingest_event(revision(), policy).unwrap();
    assert!(driver.materialize_next_context_reset().await);
    assert!(captured.memory.lock().unwrap()[0].contains("authorized Twitter User"));
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
    let sessions = SessionManager::new(Arc::new(Factory(captured.clone())), collaboration.clone());
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
        sessions: SessionManager::new(Arc::new(Factory(captured.clone())), collaboration.clone()),
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
    // The reset was persisted under the old Profile before the service restarted.
    f.store.begin_context_reset(None, "issue".into(), "issue-codex".into()).unwrap().unwrap();
    let mut config = f.config.clone();
    config.profiles.iter_mut().find(|p| p.id == "issue-codex").unwrap().model =
        Some("changed-model".into());
    let captured = Arc::new(Captured::default());
    let (github, http) = issue_context_server("Current context").await;
    let collaboration = Arc::new(CollaborationBackend::GitHub(github));
    let spec = GroupSpec::new(GroupKind::Issue, &config, &f.store, "owner/repository").unwrap();
    let driver = worker::GroupDriver {
        store: &f.store,
        config: &config,
        collaboration: &collaboration,
        spec: &spec,
        sessions: SessionManager::new(Arc::new(Factory(captured.clone())), collaboration.clone()),
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
        sessions: SessionManager::new(Arc::new(Factory(captured.clone())), collaboration.clone()),
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
        let sessions =
            SessionManager::new(Arc::new(Factory(captured.clone())), collaboration.clone());
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
