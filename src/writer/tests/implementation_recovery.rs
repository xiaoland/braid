//! Published PR identity survives a lost delivery and later request references.
use super::*;

fn request(f: &Fixture, comment: u64) -> GhWriteReceipt {
    ensure_pull_request(
        &f.config,
        &f.store,
        PullRequestEnsureRequest {
            invocation: f.invocation(),
            repository: "owner/repository".parse().unwrap(),
            comment_id: comment,
            head: Some("head".into()),
        },
    )
    .unwrap()
}
fn legacy_gap(f: &Fixture, receipt: &GhWriteReceipt) {
    let mut db = f.db();
    db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    let tx = db.transaction().unwrap();
    let (event,guid):(String,String)=tx.query_row("SELECT e.event_id,e.delivery_guid FROM implementation_requests i JOIN events e ON e.event_id=i.activation_event_id WHERE i.intent_id=?1",[&receipt.intent_id],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
    tx.execute(
        "UPDATE implementation_requests SET activation_event_id=NULL WHERE activation_event_id=?1",
        [&event],
    )
    .unwrap();
    tx.execute("DELETE FROM wake_batch_events WHERE event_id=?1", [&event]).unwrap();
    tx.execute("DELETE FROM wake_batches WHERE work_item_node_id='pr' AND NOT EXISTS(SELECT 1 FROM wake_batch_events b WHERE b.batch_id=wake_batches.batch_id)",[]).unwrap();
    tx.execute("UPDATE deliveries SET effect_event_id=NULL WHERE effect_event_id=?1", [&event])
        .unwrap();
    tx.execute("DELETE FROM events WHERE event_id=?1", [&event]).unwrap();
    tx.execute("DELETE FROM deliveries WHERE delivery_guid=?1", [guid]).unwrap();
    tx.execute(
        "UPDATE write_intents SET lifecycle='pending' WHERE intent_id=?1",
        [&receipt.intent_id],
    )
    .unwrap();
    tx.execute("UPDATE github_write_outbox SET lifecycle='pending',next_attempt_at='2000-01-01T00:00:00Z' WHERE intent_id=?1",[&receipt.intent_id]).unwrap();
    tx.commit().unwrap();
}
fn replace_source(f: &Fixture) {
    f.db().execute_batch("UPDATE provider_sessions SET lifecycle='replaced' WHERE session_id='session'; INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES('new-session','agent','codex','new-source-provider','new-context','instructions','idle','now'); INSERT INTO turns(turn_id,session_id,context_revision,trigger_kind,lifecycle) VALUES('new-turn','new-session','new-context','wake','completed');").unwrap();
}
fn activations(f: &Fixture) -> i64 {
    f.db()
        .query_row(
            "SELECT count(*) FROM events WHERE kind='assign' AND detail='pr_ensure'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn discovery_cannot_adopt_a_different_request_with_a_matching_id_prefix() {
    let f = Fixture::new(true);
    let receipt = request(&f, 7);
    let remote = Arc::new(Mutex::new(Remote {
        pr_body: Some(
            "Closes #1\n\nImplementation request: https://github.com/owner/repository/issues/comments/70"
                .into(),
        ),
        ..Remote::default()
    }));
    let (github, http) = server(remote.clone(), true).await;
    f.drain(github).await;
    let result = f.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "rejected");
    assert!(result.last_error.as_deref().unwrap().contains("unrelated PR"));
    assert_eq!(activations(&f), 0);
    assert_eq!(remote.lock().unwrap().writes, 0);
    http.abort();
}

#[test]
fn ensure_rejects_invalid_ref_components_before_accepting_an_obligation() {
    let f = Fixture::new(false);
    for head in ["topic//task", "topic/.task", "topic.lock/task"] {
        let result = ensure_pull_request(
            &f.config,
            &f.store,
            PullRequestEnsureRequest {
                invocation: Invocation::Operator,
                repository: "owner/repository".parse().unwrap(),
                comment_id: 7,
                head: Some(head.into()),
            },
        );
        assert!(result.unwrap_err().to_string().contains("invalid Git branch"), "{head}");
    }
    assert_eq!(
        f.db().query_row("SELECT COUNT(*) FROM write_intents", [], |r| r.get::<_, i64>(0)).unwrap(),
        0
    );
}

#[test]
fn ledger_errors_distinguish_retryable_contention_and_actor_loss_from_invalid_writes() {
    use crate::store::StoreError;
    for code in [rusqlite::ffi::SQLITE_BUSY, rusqlite::ffi::SQLITE_LOCKED] {
        let error = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(code),
            Some("another connection holds the ledger".into()),
        ));
        assert!(helpers::store_as_github_error(&error).is_unavailable());
    }
    for error in [StoreError::ActorUnavailable, StoreError::ActorStopped] {
        assert!(helpers::store_as_github_error(&error).is_unavailable());
    }
    let constraint = StoreError::Sqlite(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CONSTRAINT),
        Some("invalid reference".into()),
    ));
    assert!(!helpers::store_as_github_error(&constraint).is_unavailable());
    assert!(
        !helpers::store_as_github_error(&StoreError::InvalidData("source was revoked".into()))
            .is_unavailable()
    );
}

#[tokio::test]
async fn a_published_request_recovers_delivery_without_replaying_fenced_mutations() {
    let f = Fixture::new(true);
    let receipt = request(&f, 7);
    let remote = Arc::new(Mutex::new(Remote { create_pr: true, ..Remote::default() }));
    let (github, http) = server(remote.clone(), true).await;
    f.drain(github.clone()).await;
    assert_eq!(f.receipt(&receipt.request_id).lifecycle, "applied");
    let writes = remote.lock().unwrap().writes;
    assert_eq!(writes, 3);
    legacy_gap(&f, &receipt);
    replace_source(&f);
    remote.lock().unwrap().pr_body = Some("Current scope, no historical discovery marker".into());
    f.drain(github.clone()).await;
    let result = f.receipt(&receipt.request_id);
    assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
    assert!(
        f.store
            .implementation_request_receipt(receipt.intent_id.clone())
            .unwrap()
            .unwrap()
            .activation_delivered
    );
    assert_eq!(activations(&f), 1);
    assert_eq!(remote.lock().unwrap().writes, writes);
    f.db()
        .execute(
            "UPDATE github_write_outbox SET lifecycle='sending' WHERE intent_id=?1",
            [receipt.intent_id],
        )
        .unwrap();
    f.store.recover_writes().unwrap();
    f.drain(github).await;
    assert_eq!(activations(&f), 1);
    assert_eq!(remote.lock().unwrap().writes, writes);
    http.abort();
}

#[tokio::test]
async fn a_followup_comment_reuses_the_original_managed_implementation_identity() {
    let f = Fixture::new(true);
    let original = request(&f, 7);
    let remote = Arc::new(Mutex::new(Remote { create_pr: true, ..Remote::default() }));
    let (github, http) = server(remote.clone(), true).await;
    f.drain(github.clone()).await;
    legacy_gap(&f, &original);
    replace_source(&f);
    f.db().execute("UPDATE github_write_outbox SET next_attempt_at='2099-01-01T00:00:00Z' WHERE intent_id=?1",[&original.intent_id]).unwrap();
    let followup = request(&f, 8);
    f.drain(github.clone()).await;
    let result = f.receipt(&followup.request_id);
    assert_eq!(result.lifecycle, "applied", "{:?}", result.last_error);
    let plan = f.store.implementation_request_receipt(followup.intent_id).unwrap().unwrap();
    assert_eq!(plan.root_intent_id, original.intent_id);
    assert_eq!(plan.root_request_id, original.request_id);
    assert_eq!(plan.pull_request_number, Some(9));
    assert!(plan.activation_delivered);
    assert_eq!(f.receipt(&original.request_id).lifecycle, "applied");
    assert_eq!(activations(&f), 1);
    assert_eq!(remote.lock().unwrap().writes, 3);
    let next = request(&f, 8);
    assert_eq!(next.request_id, result.request_id);
    f.drain(github).await;
    assert_eq!(activations(&f), 1);
    http.abort();
}

#[tokio::test]
async fn recovery_revalidates_the_current_open_source_target_and_association() {
    for (authority, reason) in [
        (
            Authority::SourceClosed,
            "implementation source Issue is no longer the same open work item",
        ),
        (Authority::TargetClosed, "implementation request PR is no longer open"),
        (
            Authority::AssociationRemoved,
            "managed implementation is no longer directly associated with its source Issue",
        ),
    ] {
        let f = Fixture::new(true);
        let receipt = request(&f, 7);
        let remote = Arc::new(Mutex::new(Remote::default()));
        let (github, http) = server(remote.clone(), true).await;
        f.drain(github.clone()).await;
        legacy_gap(&f, &receipt);
        remote.lock().unwrap().authority = authority;
        f.drain(github).await;
        assert_rejected(&f.receipt(&receipt.request_id), reason);
        assert_eq!(activations(&f), 0);
        assert_eq!(remote.lock().unwrap().writes, 0);
        http.abort();
    }
}

#[tokio::test]
async fn recovery_cannot_replace_a_blocked_generation_or_adopt_another_issue() {
    for blocked in [false, true] {
        let f = Fixture::new(true);
        let original = request(&f, 7);
        let remote = Arc::new(Mutex::new(Remote::default()));
        let (github, http) = server(remote.clone(), true).await;
        f.drain(github.clone()).await;
        legacy_gap(&f, &original);
        if blocked {
            f.db().execute("INSERT INTO assignments(assignment_id,work_item_node_id,generation,lifecycle,assigned_at) VALUES('withdrawn','pr',1,'blocked','now')",[]).unwrap();
            f.drain(github).await;
            assert_rejected(&f.receipt(&original.request_id), "existing PR assignment is blocked");
        } else {
            f.db().execute("UPDATE github_write_outbox SET next_attempt_at='2099-01-01T00:00:00Z' WHERE intent_id=?1",[&original.intent_id]).unwrap();
            remote.lock().unwrap().authority = Authority::OtherIssue;
            let foreign = request(&f, 8);
            f.drain(github).await;
            assert_rejected(
                &f.receipt(&foreign.request_id),
                "initial PR ensure must originate from its own Issue source",
            );
        }
        assert_eq!(activations(&f), 0);
        assert_eq!(remote.lock().unwrap().writes, 0);
        http.abort();
    }
}

#[tokio::test]
async fn an_unpublished_request_still_requires_its_original_source_authority() {
    let f = Fixture::new(true);
    let receipt = request(&f, 7);
    replace_source(&f);
    let remote = Arc::new(Mutex::new(Remote { create_pr: true, ..Remote::default() }));
    let (github, http) = server(remote.clone(), true).await;
    f.drain(github).await;
    assert_rejected(
        &f.receipt(&receipt.request_id),
        "publication source was invalidated or superseded",
    );
    assert_eq!(remote.lock().unwrap().writes, 0);
    assert_eq!(activations(&f), 0);
    http.abort();
}

fn assert_rejected(receipt: &GhWriteReceipt, reason: &str) {
    assert_eq!(receipt.lifecycle, "rejected", "{receipt:?}");
    assert!(receipt.last_error.as_deref().unwrap().contains(reason), "{receipt:?}");
}
