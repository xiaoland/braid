//! Actual public comment convergence through `StoreActor` -> one outbox -> HTTP.
use super::*;
const FAULT: &str = "> **Braid Operational Status · `issue-codex`**\n\n**Provider session unavailable**\n\nThe old generation is blocked.";
fn failure(f: &Fixture) {
    f.store.enqueue_assignment_operational_status("assignment".into(), FAULT.into()).unwrap();
}
fn replacement(f: &Fixture, state: &str) {
    f.db().execute_batch("UPDATE assignments SET lifecycle='blocked'; UPDATE agent_instances SET lifecycle='blocked'; UPDATE provider_sessions SET lifecycle='blocked'; INSERT INTO assignments VALUES('replacement','item',2,'active','2002-01-01T00:00:00Z',NULL); INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) SELECT 'agent2','replacement',profile_id,profile_revision,role,'idle' FROM agent_instances WHERE agent_id='agent'; INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES('session2','agent2','codex','provider2','new','new','idle','2002-01-01T00:00:00Z');").unwrap();
    let ps = match state {
        "completed" | "starting" | "failed" => "idle",
        other => other,
    };
    f.db()
        .execute("UPDATE provider_sessions SET lifecycle=?1 WHERE session_id='session2'", [ps])
        .unwrap();
    f.db().execute("INSERT INTO turns(turn_id,session_id,provider_turn_id,context_revision,trigger_kind,lifecycle,started_at,ended_at) VALUES('turn2','session2',?1,'new','wake',?2,'2002-01-01T00:00:00Z','2002-01-01T00:00:01Z')",rusqlite::params![(state!="starting").then_some("accepted-native-turn"),state]).unwrap();
}
fn public_body(remote: &Arc<Mutex<Remote>>) -> String {
    remote.lock().unwrap().comments[0]["body"].as_str().unwrap().into()
}
fn kind(f: &Fixture) -> String {
    f.db().query_row("SELECT projection_kind FROM status_comments", [], |r| r.get(0)).unwrap()
}

#[tokio::test]
async fn a_confirmed_new_generation_updates_the_original_comment_and_never_unblocks_the_old_one() {
    let f = Fixture::new(true);
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, http) = server(remote.clone(), true).await;
    failure(&f);
    f.drain(github.clone()).await;
    assert!(public_body(&remote).contains("所属 generation：**1**"));
    replacement(&f, "completed");
    // A new actor has no in-memory transition notification: durable state alone
    // must repair a previously published failure after restart.
    let fresh =
        StoreActor::start(f.config.runtime.database().into(), f.config.runtime.backups().into())
            .unwrap();
    let owner =
        fresh.acquire_runtime_lease("runtime".into(), "fresh-publisher".into(), 3600).unwrap();
    crate::outbox::drain_one_write(
        &fresh,
        &HashMap::from([("owner/repository".into(), github.clone())]),
        &f.config,
        &owner,
    )
    .await;
    fresh.release_runtime_lease(owner).unwrap();
    drop(fresh);
    let body = public_body(&remote);
    assert!(body.contains("已由 generation 2 接替"));
    assert!(body.contains("2002-01-01T00:00:01Z"));
    assert!(body.contains("不代表代码已修改"));
    assert_eq!(kind(&f), "superseded");
    assert_eq!(
        f.db()
            .query_row(
                "SELECT lifecycle FROM assignments WHERE assignment_id='assignment'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "blocked"
    );
    failure(&f);
    f.drain(github.clone()).await;
    f.drain(github).await;
    assert_eq!(
        public_body(&remote),
        body,
        "late old-generation failure must not regress the public projection"
    );
    assert_eq!(remote.lock().unwrap().writes, 2);
    assert_eq!(remote.lock().unwrap().comments.len(), 1);
    http.abort();
}

#[tokio::test]
async fn merely_created_failed_unknown_or_blocked_successors_do_not_claim_recovery() {
    for state in ["starting", "failed", "unknown", "blocked"] {
        let f = Fixture::new(true);
        let remote = Arc::new(Mutex::new(Remote::default()));
        let (github, http) = server(remote.clone(), true).await;
        failure(&f);
        f.drain(github.clone()).await;
        let before = public_body(&remote);
        replacement(&f, state);
        f.drain(github).await;
        assert_eq!(public_body(&remote), before, "{state}");
        assert_eq!(kind(&f), "failure");
        assert_eq!(remote.lock().unwrap().writes, 1);
        http.abort();
    }
}

#[tokio::test]
async fn an_in_flight_create_keeps_its_remote_identity_when_the_desired_status_changes() {
    let f = Fixture::new(true);
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, http) = server(remote.clone(), true).await;
    failure(&f);
    let owner = f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 3600).unwrap();
    let old = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
    let created = github.create_issue_comment(1, &old.content).await.unwrap();
    replacement(&f, "completed");
    assert!(
        f.store.claim_github_write(owner.clone()).unwrap().is_none(),
        "new create waits for its unresolved predecessor"
    );
    f.store
        .finish_github_write(
            old,
            "applied",
            Some(created.id.to_string()),
            Some(created.node_id),
            None,
        )
        .unwrap();
    f.store.release_runtime_lease(owner).unwrap();
    f.drain(github).await;
    assert_eq!(remote.lock().unwrap().comments.len(), 1);
    assert!(public_body(&remote).contains("已由 generation 2 接替"));
    assert_eq!(
        f.db()
            .query_row("SELECT lifecycle FROM status_comments", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "applied"
    );
    http.abort();
}

#[tokio::test]
async fn an_uncertain_create_is_recovered_before_publishing_the_new_projection() {
    let f = Fixture::new(true);
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, http) = server(remote.clone(), true).await;
    failure(&f);
    let owner = f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 3600).unwrap();
    let old = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
    github.create_issue_comment(1, &old.content).await.unwrap(); // Remote applied; local ACK was lost.
    f.store.release_runtime_lease(owner).unwrap();
    f.store.recover_writes().unwrap();
    replacement(&f, "completed");
    f.drain(github.clone()).await;
    assert_eq!(
        remote.lock().unwrap().writes,
        1,
        "recovery observes the first create, not another POST"
    );
    f.drain(github).await;
    assert_eq!(remote.lock().unwrap().comments.len(), 1);
    assert_eq!(remote.lock().unwrap().writes, 2);
    assert!(public_body(&remote).contains("已由 generation 2 接替"));
    http.abort();
}

#[tokio::test]
async fn ambiguous_create_blocks_only_that_comments_successor_after_restart() {
    let f = Fixture::new(true);
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, http) = server(remote.clone(), true).await;
    failure(&f);
    let owner = f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 3600).unwrap();
    let old = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
    github.create_issue_comment(1, &old.content).await.unwrap();
    {
        let mut state = remote.lock().unwrap();
        let mut twin = state.comments[0].clone();
        twin["id"] = json!(100);
        twin["node_id"] = json!("other-comment");
        state.comments.push(twin);
    }
    f.store.release_runtime_lease(owner).unwrap();
    replacement(&f, "completed");
    let f = f.reopen();
    f.store.recover_writes().unwrap();
    f.drain(github.clone()).await;
    assert_eq!(
        f.db()
            .query_row(
                "SELECT lifecycle FROM github_write_outbox WHERE intent_id=?1",
                [old.intent_id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "ambiguous"
    );
    f.store
        .enqueue_assignment_operational_status("replacement".into(), "new generation fault".into())
        .unwrap();
    f.drain(github.clone()).await;
    f.drain(github).await;
    let state = remote.lock().unwrap();
    assert_eq!(
        state.writes, 2,
        "one original POST and one independent generation's POST; no third POST or guessed PATCH"
    );
    assert_eq!(state.comments.len(), 3);
    assert!(state.comments[2]["body"].as_str().unwrap().contains("new generation fault"));
    assert_eq!(
        f.db()
            .query_row(
                "SELECT lifecycle FROM status_comments WHERE assignment_generation=1",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "ambiguous"
    );
    http.abort();
}

#[tokio::test]
async fn repeated_identical_failures_do_not_orphan_receipts_and_a_new_cycle_can_publish_again() {
    let f = Fixture::new(true);
    let remote = Arc::new(Mutex::new(Remote::default()));
    let (github, http) = server(remote.clone(), true).await;
    for (outcome, stamp) in [
        ("failed", "2001-01-01T00:00:00Z"),
        ("completed", "2002-01-01T00:00:00Z"),
        ("failed", "2003-01-01T00:00:00Z"),
        ("completed", "2004-01-01T00:00:00Z"),
    ] {
        f.db().execute("UPDATE turns SET lifecycle=?1,provider_turn_id='accepted',ended_at=?2 WHERE turn_id='turn'",rusqlite::params![outcome,stamp]).unwrap();
        if outcome == "failed" {
            f.store.enqueue_operational_status("turn".into(), FAULT.into()).unwrap();
            f.store.enqueue_operational_status("turn".into(), FAULT.into()).unwrap();
        }
        f.drain(github.clone()).await;
        assert_eq!(kind(&f), if outcome == "failed" { "failure" } else { "recovered" });
    }
    assert_eq!(remote.lock().unwrap().comments.len(), 1);
    assert_eq!(remote.lock().unwrap().writes, 4);
    assert!(public_body(&remote).contains("2004-01-01T00:00:00Z"));
    assert_eq!(f.db().query_row("SELECT count(*) FROM status_comments s JOIN github_write_outbox o ON o.intent_id=s.write_intent_id WHERE o.lifecycle='applied' AND s.lifecycle='applied'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    http.abort();
}
