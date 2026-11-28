//! End-to-end local handoff uses production actor, outbox, canonical HTTP reads and boundary assembly.
use super::*;

fn bind_pr(f: &Fixture) {
    f.db().execute_batch("INSERT INTO work_items(node_id,repository_node_id,kind,number,state,context_revision,observed_at) VALUES ('pr','repo','pr',9,'open','old-pr-context','now'); INSERT INTO assignments VALUES ('pr-assignment','pr',1,'active','now',NULL); INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) SELECT 'pr-agent','pr-assignment',profile_id,profile_revision,'pr_implementation_agent','idle' FROM agent_instances WHERE agent_id='agent'; INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES ('pr-session','pr-agent','codex','pr-provider','old-pr-context','instructions','idle','now');").unwrap();
}
fn handoff(f: &Fixture, key: &str) -> Result<GhWriteReceipt> {
    super::super::handoff(
        &f.config,
        &f.store,
        &HandoffRequest {
            invocation: Invocation::Agent("turn".into()),
            target: "owner/repository#9".parse().unwrap(),
            request_id: key.into(),
        },
    )
}
fn count(f: &Fixture, table: &str) -> i64 {
    f.db().query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap()
}
fn runnable(f: &Fixture) {
    f.db().execute("UPDATE wake_batches SET quiet_deadline='2000-01-01T00:00:00Z'", []).unwrap();
    f.store.advance_scheduler().unwrap();
}
fn claim(f: &Fixture) -> Option<crate::store::TurnClaim> {
    f.store
        .claim_runnable_turn(
            "pr".into(),
            "issue-codex".into(),
            vec!["pr-provider".into(), "new-pr-provider".into()],
        )
        .unwrap()
}

#[tokio::test]
async fn handoffs_debounce_together_and_redelivery_does_not_duplicate_their_inputs() {
    let f = Fixture::new(true);
    bind_pr(&f);
    let a = handoff(&f, "stable").unwrap();
    assert_eq!(a.request_id, handoff(&f, "stable").unwrap().request_id);
    let (github, task) =
        body_edit::body_server(Arc::new(Mutex::new(body_edit::BodyRemote::new(false)))).await;
    f.drain(github.clone()).await;
    assert_eq!(f.receipt(&a.request_id).lifecycle, "applied");
    assert_eq!(count(&f, "events"), 1);
    assert_eq!(count(&f, "wake_batches"), 1);
    assert_eq!(
        f.db().query_row("SELECT urgent FROM wake_batches", [], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
    let b = handoff(&f, "second").unwrap();
    f.drain(github).await;
    assert_eq!(f.receipt(&b.request_id).lifecycle, "applied");
    assert_eq!(count(&f, "wake_batches"), 1); // merge existing pending
    runnable(&f);
    let turn = claim(&f).unwrap();
    assert_eq!(turn.references.len(), 2);
    task.abort();
}

fn add_runnable_event(f: &Fixture, id: &str, reference: &str, urgent: bool) {
    let mut connection = f.db();
    let tx = connection.transaction().unwrap();
    tx.execute(
        "INSERT INTO events(event_id,work_item_node_id,kind,origin,reference,lifecycle,observed_at)
         VALUES (?1,'pr','wake','human',?2,'pending','now')",
        rusqlite::params![id, reference],
    )
    .unwrap();
    tx.execute(
        "INSERT INTO wake_batches(batch_id,work_item_node_id,event_count,quiet_deadline,urgent,lifecycle,created_at,updated_at)
         VALUES (?1,'pr',1,'now',?2,'runnable','now','now')",
        rusqlite::params![id, urgent],
    ).unwrap();
    tx.execute("INSERT INTO wake_batch_events VALUES (?1,?1,0)", [id]).unwrap();
    tx.commit().unwrap();
}

#[tokio::test]
async fn accepted_handoff_waits_for_next_turn_and_survives_unavailable_publisher() {
    for associated in [true, false] {
        let f = Fixture::new(true);
        bind_pr(&f);
        add_runnable_event(&f, "running", "Already running", false);
        let running = claim(&f).unwrap();
        f.store.mark_turn_started(running.turn_id.clone(), "native-running".into()).unwrap();

        let receipt = handoff(&f, "arrives-during-steer").unwrap();
        let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
        let (github, task) = body_edit::body_server(remote.clone()).await;
        add_runnable_event(&f, "steer", "Emergency human input", true);
        let steer = f.store.claim_urgent_steer(running.turn_id.clone()).unwrap().unwrap();
        assert_eq!(steer.turn.references, ["Emergency human input"]);

        // The provider is replying to that exact snapshot when the handoff joins its batch.
        f.drain(github.clone()).await;
        assert_eq!(f.receipt(&receipt.request_id).lifecycle, "applied");
        let facts: (String, i64, String, i64, i64, Option<String>) = f.db().query_row(
            "SELECT h.source_repository,h.source_number,h.target_assignment_id,h.target_generation,
                    e.next_turn_only,p.target_assignment_id
             FROM handoff_inputs h JOIN events e ON e.event_id=h.event_id
             JOIN pr_handoffs p ON p.event_id=h.event_id",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
        ).unwrap();
        assert_eq!(facts, ("owner/repository".into(), 1, "pr-assignment".into(), 1, 1, None));

        // Remove publisher table names altogether. A query that still joins them errors.
        f.db()
            .execute_batch(
                "ALTER TABLE pr_handoffs RENAME TO unavailable_handoff_receipts;
             ALTER TABLE write_intents RENAME TO unavailable_write_intents;
             ALTER TABLE github_write_outbox RENAME TO unavailable_write_outbox;",
            )
            .unwrap();
        assert!(f.store.gh_write_receipt(receipt.request_id).is_err());
        assert!(f.store.claim_urgent_steer(running.turn_id.clone()).unwrap().is_none());
        f.store.consume_steer_batch(&steer).unwrap();
        assert!(f.store.claim_urgent_steer(running.turn_id.clone()).unwrap().is_none());
        assert!(claim(&f).is_none());
        f.store.mark_turn_terminal(running.turn_id, "completed".into()).unwrap();

        remote.lock().unwrap().associated = associated;
        let next = claim(&f).expect("handoff survives late steer settlement");
        assert_eq!(next.references.len(), 1);
        assert!(next.references[0].contains("Explicit Issue handoff"));
        let recorder = RecordingSession::new();
        let backend = crate::collaboration::CollaborationBackend::GitHub(github);
        if associated {
            send_prepared(&f, &backend, next, &recorder).await;
            let inputs = recorder.inputs.lock().unwrap();
            assert_eq!(inputs.len(), 1);
            assert!(inputs[0].contains("Explicit Issue handoff"));
            assert!(inputs[0].contains("Current canonical Context"));
        } else {
            let prepared = crate::group::input::prepare_input(
                &f.config,
                &backend,
                &f.store,
                f.config.profile("issue-codex").unwrap(),
                next,
            )
            .await
            .unwrap();
            assert!(prepared.is_none(), "canonical disassociation retires the handoff");
            let lifecycle: String = f
                .db()
                .query_row("SELECT lifecycle FROM events WHERE detail='pr_handoff'", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(lifecycle, "consumed");
        }
        assert!(claim(&f).is_none(), "the handoff must not surface twice");
        task.abort();
    }
}

#[tokio::test]
async fn handoff_rejects_local_authority_and_generation_changes() {
    let unassigned = Fixture::new(true);
    unassigned.db().execute("INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at) VALUES ('pr','repo','pr',9,'open','now')", []).unwrap();
    assert!(
        handoff(&unassigned, "no-assignment")
            .unwrap_err()
            .to_string()
            .contains("handoff target needs a current active schedulable PR generation")
    );
    for sql in [
        "UPDATE assignments SET lifecycle='blocked' WHERE assignment_id='pr-assignment'",
        "UPDATE assignments SET lifecycle='finalizing' WHERE assignment_id='pr-assignment'",
        "DELETE FROM provider_sessions WHERE session_id='pr-session'",
        "INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at,resume_count,last_resumed_at,last_resume_error) SELECT 'latest-pr-session',agent_id,provider_kind,'latest-pr-provider',context_revision,instruction_revision,'blocked',started_at,resume_count,last_resumed_at,last_resume_error FROM provider_sessions WHERE session_id='pr-session'",
        "UPDATE work_items SET state='closed' WHERE node_id='pr'",
        "UPDATE work_items SET state='merged' WHERE node_id='pr'",
        "UPDATE work_items SET state='closed' WHERE node_id='item'",
    ] {
        let f = Fixture::new(true);
        bind_pr(&f);
        f.db().execute(sql, []).unwrap();
        let expected = if sql.contains("node_id='item'") {
            "handoff requires an active Issue Agent on its OPEN Issue"
        } else {
            "handoff target needs a current active schedulable PR generation"
        };
        let error = handoff(&f, "reject").unwrap_err();
        assert!(error.to_string().contains(expected), "{sql}: {error}");
        assert_eq!(count(&f, "pr_handoffs"), 0);
    }
    let f = Fixture::new(true);
    bind_pr(&f);
    let a = handoff(&f, "frozen").unwrap();
    f.db().execute_batch("UPDATE assignments SET lifecycle='retired' WHERE assignment_id='pr-assignment'; INSERT INTO assignments VALUES ('new-pr','pr',2,'active','now',NULL); UPDATE agent_instances SET assignment_id='new-pr' WHERE agent_id='pr-agent';").unwrap();
    assert_eq!(handoff(&f, "frozen").unwrap().request_id, a.request_id);
    let (github, task) =
        body_edit::body_server(Arc::new(Mutex::new(body_edit::BodyRemote::new(false)))).await;
    f.drain(github).await;
    assert_eq!(f.receipt(&a.request_id).lifecycle, "rejected");
    assert_eq!(count(&f, "events"), 0);
    task.abort();
}

#[tokio::test]
async fn handoff_remote_validation_never_schedules_before_confirmation() {
    for (mode, lifecycle, reason) in [
        ("unrelated", "rejected", "handoff requires GitHub-confirmed direct association"),
        ("closed", "rejected", "handoff requires an OPEN Issue and OPEN unmerged PR"),
        ("closed-target", "rejected", "handoff requires an OPEN Issue and OPEN unmerged PR"),
        ("limited", "pending", "429"),
        ("old-source", "rejected", "publication source was invalidated or superseded"),
    ] {
        let f = Fixture::new(true);
        bind_pr(&f);
        let a = handoff(&f, mode).unwrap();
        let mut state = body_edit::BodyRemote::new(false);
        if mode == "unrelated" {
            state.associated = false;
        } // body still contains closing/reference text
        if mode == "closed" {
            state.snapshot.state = "closed".into();
        }
        if mode == "closed-target" {
            state.pr_closed = true;
        }
        if mode == "limited" {
            state.limited = true;
        }
        if mode == "old-source" {
            f.db()
                .execute("UPDATE assignments SET generation=2 WHERE assignment_id='assignment'", [])
                .unwrap();
        }
        let (github, task) = body_edit::body_server(Arc::new(Mutex::new(state))).await;
        f.drain(github).await;
        let receipt = f.receipt(&a.request_id);
        assert_eq!(receipt.lifecycle, lifecycle, "{mode}: {receipt:?}");
        assert!(receipt.last_error.as_deref().unwrap().contains(reason), "{mode}: {receipt:?}");
        assert_eq!(count(&f, "events"), 0);
        assert_eq!(count(&f, "wake_batches"), 0);
        task.abort();
    }
}

#[tokio::test]
async fn applied_issue_design_is_materialized_at_handoff_safe_boundary_not_from_cached_context() {
    let f = Fixture::new(true);
    bind_pr(&f);
    let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
    let (github, task) = body_edit::body_server(remote.clone()).await;
    let edit = body_edit::accept(&f, &github).await;
    assert!(handoff(&f, "too-soon").unwrap_err().to_string().contains("not confirmed applied"));
    f.drain(github.clone()).await;
    assert_eq!(f.receipt(&edit.request_id).lifecycle, "applied");
    let request = handoff(&f, "published-design").unwrap();
    f.drain(github.clone()).await;
    assert_eq!(f.receipt(&request.request_id).lifecycle, "applied");
    runnable(&f);
    let turn = claim(&f).unwrap();
    let mut reference = crate::group::provider::render_event_references(&f.config, &turn);
    let collaboration = crate::collaboration::CollaborationBackend::GitHub(github);
    crate::group::input::append_current_context(
        f.config.runtime.database(),
        &collaboration,
        &f.store,
        f.config.profile("issue-codex").unwrap(),
        &turn,
        &mut reference,
    )
    .await
    .unwrap();
    assert!(reference.contains("Scope, verification, risks"), "{reference}");
    assert!(reference.contains("Current canonical Context"));
    assert!(!reference.contains("old-pr-context"));
    task.abort();
}

#[test]
fn handoff_same_key_cannot_change_target_or_impersonate_issue_agent() {
    let f = Fixture::new(true);
    bind_pr(&f);
    handoff(&f, "stable").unwrap();
    assert!(
        super::super::handoff(
            &f.config,
            &f.store,
            &HandoffRequest {
                invocation: Invocation::Agent("turn".into()),
                target: "owner/repository#10".parse().unwrap(),
                request_id: "stable".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already used for a different payload")
    );
    f.db().execute_batch("UPDATE agent_instances SET role='pr_implementation_agent' WHERE agent_id='agent'; UPDATE work_items SET kind='pr',number=10 WHERE node_id='item';").unwrap();
    assert!(
        handoff(&f, "pr-source")
            .unwrap_err()
            .to_string()
            .contains("handoff requires an Issue Agent")
    );
}

#[tokio::test]
async fn handoff_transaction_rechecks_source_fence_before_any_wake_or_applied_receipt() {
    let f = Fixture::new(true);
    bind_pr(&f);
    let a = handoff(&f, "fence-race").unwrap();
    let (github, task) =
        body_edit::body_server(Arc::new(Mutex::new(body_edit::BodyRemote::new(false)))).await;
    let owner = f.store.acquire_runtime_lease("runtime".into(), "test".into(), 3600).unwrap();
    let write = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
    let mut mutation_possible = false;
    assert!(
        super::super::execute(&f.config, &f.store, &github, &write, &mut mutation_possible)
            .await
            .is_ok()
    );
    f.db()
        .execute("UPDATE assignments SET generation=2 WHERE assignment_id='assignment'", [])
        .unwrap();
    f.store.finish_github_write(write, "applied", None, None, None).unwrap();
    assert_eq!(f.receipt(&a.request_id).lifecycle, "rejected");
    assert_eq!(count(&f, "events"), 0);
    assert_eq!(count(&f, "wake_batches"), 0);
    f.store.release_runtime_lease(owner).unwrap();
    task.abort();
}

#[tokio::test]
async fn refresh_failure_defers_and_merges_an_arriving_handoff_without_losing_inputs() {
    let f = Fixture::new(true);
    bind_pr(&f);
    handoff(&f, "first").unwrap();
    let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
    let (github, task) = body_edit::body_server(remote.clone()).await;
    f.drain(github.clone()).await;
    runnable(&f);
    let first = claim(&f).unwrap();
    handoff(&f, "arrives-during-refresh").unwrap();
    f.drain(github.clone()).await;
    add_input(&f, "wake", "HumanWake arriving during network failure");
    remote.lock().unwrap().limited = true;
    let backend = crate::collaboration::CollaborationBackend::GitHub(github);
    let mut input = String::new();
    let result = crate::group::input::append_current_context(
        f.config.runtime.database(),
        &backend,
        &f.store,
        f.config.profile("issue-codex").unwrap(),
        &first,
        &mut input,
    )
    .await;
    assert!(
        matches!(result, Err(ref error) if error.to_string().contains("503")),
        "expected canonical HTTP 503"
    );
    assert!(input.is_empty());
    f.store.defer_turn_claim(first.turn_id).unwrap();
    assert_eq!(
        f.db()
            .query_row(
                "SELECT COUNT(*) FROM wake_batches WHERE lifecycle IN ('pending','runnable')",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let retry = claim(&f).unwrap();
    assert_eq!(retry.references.len(), 3);
    remote.lock().unwrap().limited = false;
    // A fresh HTTP client models process restart after the transient failure;
    // the original client correctly retains its installation cooldown.
    let (recovered_github, recovered_task) = body_edit::body_server(remote.clone()).await;
    let backend = crate::collaboration::CollaborationBackend::GitHub(recovered_github);
    let recorder = RecordingSession::new();
    send_prepared(&f, &backend, retry, &recorder).await;
    assert_eq!(recorder.inputs.lock().unwrap().len(), 1);
    assert!(recorder.inputs.lock().unwrap()[0].contains("Current canonical Context"));
    assert_eq!(count(&f, "pr_handoffs"), 2);
    assert!(
        recorder.inputs.lock().unwrap()[0].contains("HumanWake arriving during network failure")
    );
    recovered_task.abort();
    task.abort();
}

#[tokio::test]
async fn uncertain_issue_design_is_not_handed_off_as_published() {
    let f = Fixture::new(true);
    bind_pr(&f);
    let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
    let (github, task) = body_edit::body_server(remote.clone()).await;
    let edit = body_edit::accept(&f, &github).await;
    remote.lock().unwrap().lose_response = true;
    f.drain(github).await;
    assert_eq!(f.receipt(&edit.request_id).lifecycle, "uncertain");
    assert!(
        handoff(&f, "must-wait")
            .unwrap_err()
            .to_string()
            .contains("Issue design edit is not confirmed applied")
    );
    assert_eq!(count(&f, "pr_handoffs"), 0);
    task.abort();
}

/// Provider-boundary recorder: the production send/observe path must record a
/// native start, not merely make the batch claimable again.
struct RecordingSession {
    events: tokio::sync::broadcast::Sender<crate::agent_session::SessionEvent>,
    inputs: Mutex<Vec<String>>,
}
impl RecordingSession {
    fn new() -> Self {
        Self { events: tokio::sync::broadcast::channel(8).0, inputs: Mutex::new(Vec::new()) }
    }
}
#[async_trait::async_trait]
impl crate::agent_session::AgentSession for RecordingSession {
    fn events(&self) -> tokio::sync::broadcast::Receiver<crate::agent_session::SessionEvent> {
        self.events.subscribe()
    }
    fn is_unavailable(&self) -> bool {
        false
    }
    async fn close(&self) -> std::result::Result<(), crate::agent_session::SessionError> {
        Ok(())
    }
    async fn steer_user_msg(
        &self,
        _: String,
        _: &str,
    ) -> std::result::Result<crate::agent_session::SendResult, crate::agent_session::SessionError>
    {
        panic!("handoff must not steer")
    }
    async fn send_user_msg(
        &self,
        msg: String,
        steering: bool,
    ) -> std::result::Result<crate::agent_session::SendResult, crate::agent_session::SessionError>
    {
        assert!(!steering);
        self.inputs.lock().unwrap().push(msg);
        self.events
            .send(crate::agent_session::SessionEvent::TurnStarted {
                provider_turn_id: "native-start".into(),
            })
            .unwrap();
        Ok(crate::agent_session::SendResult::Started)
    }
}
async fn send_prepared(
    f: &Fixture,
    backend: &crate::collaboration::CollaborationBackend,
    turn: crate::store::TurnClaim,
    recorder: &RecordingSession,
) {
    use crate::agent_session::AgentSession;
    let (turn, input) = crate::group::input::prepare_input(
        &f.config,
        backend,
        &f.store,
        f.config.profile("issue-codex").unwrap(),
        turn,
    )
    .await
    .unwrap()
    .expect("surviving input");
    let mut events = recorder.events();
    let native = crate::group::turn_start::send_and_observe(
        &f.store,
        &turn.turn_id,
        &turn.provider_session_id,
        recorder,
        &mut events,
        input,
    )
    .await
    .unwrap();
    assert_eq!(native.as_deref(), Some("native-start"));
    f.store.mark_turn_terminal(turn.turn_id, "completed".into()).unwrap();
}

fn add_input(f: &Fixture, kind: &str, reference: &str) {
    f.db().execute("INSERT INTO events(event_id,work_item_node_id,kind,detail,origin,reference,lifecycle,observed_at) VALUES ('other-input','pr',?1,NULL,'human',?2,'pending','now')", rusqlite::params![kind, reference]).unwrap();
    f.db().execute_batch("INSERT INTO wake_batch_events SELECT batch_id,'other-input',(SELECT COALESCE(MAX(ordinal)+1,0) FROM wake_batch_events be WHERE be.batch_id=b.batch_id) FROM wake_batches b WHERE lifecycle IN ('pending','runnable'); UPDATE wake_batches SET event_count=event_count+1,urgent=1 WHERE lifecycle IN ('pending','runnable');").unwrap();
    // A trusted HumanWake is the durable source of urgency.
    f.db().execute("UPDATE events SET trusted_mention=1 WHERE event_id='other-input'", []).unwrap();
}

#[tokio::test]
async fn canonical_invalid_handoff_retires_only_itself_and_survivors_really_send() {
    for mode in ["association-removed", "source-closed", "target-closed", "new-generation"] {
        let f = Fixture::new(true);
        bind_pr(&f);
        let receipt = handoff(&f, mode).unwrap();
        let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
        let (mut github, mut task) = body_edit::body_server(remote.clone()).await;
        f.drain(github.clone()).await;
        assert_eq!(f.receipt(&receipt.request_id).lifecycle, "applied");
        let reference = match mode {
            "target-closed" => "Legitimate Finalization cleanup",
            "new-generation" => "New generation activation Wake",
            _ => "Legitimate HumanWake",
        };
        add_input(&f, if mode == "target-closed" { "lifecycle" } else { "wake" }, reference);
        match mode {
            "association-removed" => remote.lock().unwrap().associated = false,
            "source-closed" => remote.lock().unwrap().snapshot.state = "closed".into(),
            "target-closed" => {
                remote.lock().unwrap().pr_closed = true;
                f.db().execute_batch("UPDATE work_items SET state='closed' WHERE node_id='pr'; UPDATE assignments SET lifecycle='finalizing' WHERE assignment_id='pr-assignment'; UPDATE agent_instances SET lifecycle='finalizing' WHERE agent_id='pr-agent';").unwrap();
            }
            "new-generation" => {
                f.db().execute_batch("UPDATE assignments SET lifecycle='retired' WHERE assignment_id='pr-assignment'; INSERT INTO assignments VALUES ('new-pr','pr',2,'active','now',NULL); UPDATE agent_instances SET lifecycle='retired' WHERE agent_id='pr-agent'; UPDATE provider_sessions SET lifecycle='retired' WHERE session_id='pr-session'; INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) SELECT 'new-pr-agent','new-pr',profile_id,profile_revision,role,'idle' FROM agent_instances WHERE agent_id='pr-agent'; INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES ('new-pr-session','new-pr-agent','codex','new-pr-provider','new-pr-context','instructions','idle','now');").unwrap();
                // Retirement has local durable proof; a surviving new turn
                // still needs complete current Context before native input.
                remote.lock().unwrap().limited = true;
            }
            _ => unreachable!(),
        }
        runnable(&f);
        let turn = claim(&f).unwrap();
        assert_eq!(turn.references.len(), 2);
        if mode == "target-closed" {
            assert_eq!(turn.trigger_kind, "finalization");
        }
        if mode == "new-generation" {
            assert_eq!(turn.provider_session_id, "new-pr-provider");
        }
        let turn = if mode == "new-generation" {
            let backend = crate::collaboration::CollaborationBackend::GitHub(github.clone());
            let error = crate::group::input::prepare_input(
                &f.config,
                &backend,
                &f.store,
                f.config.profile("issue-codex").unwrap(),
                turn.clone(),
            )
            .await
            .unwrap_err();
            assert!(format!("{error:#}").contains("503"), "{error:#}");
            f.store.defer_turn_claim(turn.turn_id).unwrap();
            remote.lock().unwrap().limited = false;
            // The failed transport keeps its real installation cooldown. A
            // fresh client models recovery/restart without sleeping in a test.
            task.abort();
            let recovered = body_edit::body_server(remote.clone()).await;
            github = recovered.0;
            task = recovered.1;
            claim(&f).unwrap()
        } else {
            turn
        };
        let batch = turn.batch_id.clone();
        let recorder = RecordingSession::new();
        let backend = crate::collaboration::CollaborationBackend::GitHub(github);
        send_prepared(&f, &backend, turn, &recorder).await;
        let inputs = recorder.inputs.lock().unwrap();
        assert_eq!(inputs.len(), 1, "{mode}");
        assert!(inputs[0].contains(reference), "{mode}: {}", inputs[0]);
        assert!(!inputs[0].contains("Explicit Issue handoff"), "{mode}");
        assert!(inputs[0].contains("Current canonical Context"), "{mode}");
        assert_eq!(f.receipt(&receipt.request_id).lifecycle, "applied");
        assert_eq!(count(&f, "pr_handoffs"), 1);
        assert_eq!(
            f.db()
                .query_row("SELECT lifecycle FROM events WHERE detail='pr_handoff'", [], |r| r
                    .get::<_, String>(
                    0
                ))
                .unwrap(),
            "consumed"
        );
        assert_eq!(
            f.db()
                .query_row(
                    "SELECT event_count FROM wake_batches WHERE batch_id=?1",
                    [&batch],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        f.store.advance_scheduler().unwrap();
        assert!(claim(&f).is_none(), "no duplicate finalization/wake: {mode}");
        task.abort();
    }
}

#[tokio::test]
async fn invalid_handoff_only_batch_ends_and_returns_claim_without_losing_concurrent_input() {
    let f = Fixture::new(true);
    bind_pr(&f);
    handoff(&f, "empty").unwrap();
    let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
    let (github, task) = body_edit::body_server(remote.clone()).await;
    f.drain(github.clone()).await;
    runnable(&f);
    let turn = claim(&f).unwrap();
    let empty_batch = turn.batch_id.clone();
    remote.lock().unwrap().associated = false;
    // Another ordinary input arriving during refresh owns its own debounce.
    f.db().execute_batch("INSERT INTO wake_batches(batch_id,work_item_node_id,event_count,quiet_deadline,urgent,lifecycle,created_at,updated_at) VALUES ('next','pr',0,'2099-01-01T00:00:00Z',0,'pending','now','now')").unwrap();
    add_input(&f, "wake", "Arrived during refresh");
    let backend = crate::collaboration::CollaborationBackend::GitHub(github);
    assert!(
        crate::group::input::prepare_input(
            &f.config,
            &backend,
            &f.store,
            f.config.profile("issue-codex").unwrap(),
            turn
        )
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(
        f.db()
            .query_row(
                "SELECT event_count FROM wake_batches WHERE batch_id=?1",
                [&empty_batch],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        f.db()
            .query_row("SELECT COUNT(*) FROM turns WHERE batch_id=?1", [&empty_batch], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        f.db()
            .query_row(
                "SELECT lifecycle FROM provider_sessions WHERE session_id='pr-session'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "idle"
    );
    assert!(claim(&f).is_none()); // surviving arrival is still debouncing
    runnable(&f);
    let recorder = RecordingSession::new();
    send_prepared(&f, &backend, claim(&f).unwrap(), &recorder).await;
    assert!(recorder.inputs.lock().unwrap()[0].contains("Arrived during refresh"));
    assert!(claim(&f).is_none());
    task.abort();
}

#[tokio::test]
async fn retirement_is_fenced_atomic_and_preserves_other_handoff_and_historical_source() {
    use crate::store::handoff_inputs::{RetiredInput, RetirementReason};
    let f = Fixture::new(true);
    bind_pr(&f);
    let first = handoff(&f, "first").unwrap();
    let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
    let (github, task) = body_edit::body_server(remote).await;
    f.drain(github.clone()).await;
    handoff(&f, "survivor").unwrap();
    f.drain(github.clone()).await;
    runnable(&f);
    let turn = claim(&f).unwrap();
    let event: String = f
        .db()
        .query_row("SELECT event_id FROM pr_handoffs WHERE intent_id=?1", [&first.intent_id], |r| {
            r.get(0)
        })
        .unwrap();
    let input =
        RetiredInput { event_id: event.clone(), reason: RetirementReason::SourceNotOpenAssociated };
    // A bad second membership must roll back the first retirement as well.
    assert!(
        f.store
            .retire_handoff_inputs(
                turn.clone(),
                vec![
                    input.clone(),
                    RetiredInput {
                        event_id: "not-a-member".into(),
                        reason: RetirementReason::TargetClosed
                    }
                ]
            )
            .unwrap_err()
            .to_string()
            .contains("handoff retirement membership changed")
    );
    assert_eq!(
        f.db()
            .query_row("SELECT lifecycle FROM events WHERE event_id=?1", [&event], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "pending"
    );
    let remaining =
        f.store.retire_handoff_inputs(turn.clone(), vec![input.clone()]).unwrap().unwrap();
    assert_eq!(remaining.references.len(), 1);
    // Applied delivery is historical; later source retirement/generation does
    // not revoke it while the source Issue remains OPEN and associated.
    f.db().execute("UPDATE assignments SET lifecycle='retired',generation=2 WHERE assignment_id='assignment'", []).unwrap();
    let recorder = RecordingSession::new();
    let backend = crate::collaboration::CollaborationBackend::GitHub(github);
    send_prepared(&f, &backend, remaining, &recorder).await;
    assert!(recorder.inputs.lock().unwrap()[0].contains("Current canonical Context"));
    // No mutation using a stale claim after the provider boundary.
    assert!(matches!(
        f.store.retire_handoff_inputs(turn, vec![input]),
        Err(crate::store::StoreError::Sqlite(rusqlite::Error::QueryReturnedNoRows))
    ));
    assert_eq!(count(&f, "pr_handoffs"), 2);
    assert_eq!(f.receipt(&first.request_id).lifecycle, "applied");
    task.abort();
}
