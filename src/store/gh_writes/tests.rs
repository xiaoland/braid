//! Ledger invariants through the production actor and full migration chain.
use super::{ImplementationProgress, NewGhWriteIntent, NewImplementationRequest};
use crate::store::{
    MIGRATIONS, StoreActor, StoreError, apply_one, configure_connection, open_read_write,
};
use rusqlite::{Connection, params};
use std::path::PathBuf;

struct Database {
    root: PathBuf,
    path: PathBuf,
}
impl Database {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("braid-publication-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        Self { path: root.join("state.sqlite3"), root }
    }
    fn store(&self) -> StoreActor {
        let store = StoreActor::start(self.path.clone(), self.root.join("backups")).unwrap();
        store.apply().unwrap();
        store
    }
    fn connect(&self) -> Connection {
        Connection::open(&self.path).unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn intent(operation: &'static str) -> NewGhWriteIntent {
    NewGhWriteIntent {
        request_key: "stable-key".into(),
        operation,
        repository: "owner/repository".into(),
        target: "owner/repository#1".into(),
        profile_id: "profile".into(),
        role: "pending_validation".into(),
        payload: "raw body".into(),
        request_digest: "a".repeat(64),
        source: None,
    }
}
fn invalid_data(error: &StoreError, expected: &str) {
    assert!(
        matches!(error, StoreError::InvalidData(message) if message.contains(expected)),
        "wrong failure: {error}"
    );
}

#[test]
fn offline_acceptance_is_atomic_idempotent_and_publicly_queryable() {
    let db = Database::new();
    let store = db.store();
    let write = intent("comment_create");
    let first = store.prepare_gh_write(write.clone()).unwrap();
    assert!(first.request_id.starts_with("request-"));
    assert_ne!(first.request_id, first.intent_id);
    assert_eq!(first.request_id, store.prepare_gh_write(write.clone()).unwrap().request_id);
    let mut conflict = write;
    conflict.request_digest = "b".repeat(64);
    invalid_data(
        &store.prepare_gh_write(conflict).unwrap_err(),
        "already used for a different payload",
    );
    drop(store);
    let reopened = db.store();
    assert_eq!(
        reopened.gh_write_receipt(first.request_id).unwrap().unwrap().intent_id,
        first.intent_id
    );
    for table in ["write_intents", "github_write_outbox"] {
        assert_eq!(
            db.connect()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}

#[test]
fn retry_deadline_survives_actor_restart_and_stale_attempt_cannot_settle() {
    let db = Database::new();
    let store = db.store();
    let owner =
        store.acquire_runtime_lease("runtime".into(), "before-restart".into(), 3600).unwrap();
    store.prepare_gh_write(intent("comment_create")).unwrap();
    let mut first = store.claim_github_write(owner.clone()).unwrap().unwrap();
    first.retry_after = 3600;
    store
        .finish_github_write(first, "pending", None, None, Some("limit before mutation".into()))
        .unwrap();
    store.release_runtime_lease(owner).unwrap();
    drop(store);
    let reopened = db.store();
    let owner =
        reopened.acquire_runtime_lease("runtime".into(), "after-restart".into(), 3600).unwrap();
    assert!(reopened.claim_github_write(owner.clone()).unwrap().is_none());
    db.connect()
        .execute("UPDATE github_write_outbox SET next_attempt_at='2000-01-01T00:00:00Z'", [])
        .unwrap();
    let second = reopened.claim_github_write(owner.clone()).unwrap().unwrap();
    assert_eq!(reopened.recover_writes().unwrap(), 1);
    let third = reopened.claim_github_write(owner.clone()).unwrap().unwrap();
    assert_eq!(third.lifecycle, "uncertain");
    invalid_data(
        &reopened.finish_github_write(second, "applied", None, None, None).unwrap_err(),
        "stale outbox attempt",
    );
    reopened.finish_github_write(third, "applied", Some("123".into()), None, None).unwrap();
    assert!(reopened.claim_github_write(owner).unwrap().is_none());
}

#[test]
fn expired_or_replaced_runtime_cannot_claim_or_settle() {
    for change in [
        "UPDATE owner_leases SET expires_at='2000-01-01T00:00:00Z'",
        "UPDATE owner_leases SET owner_id='replacement',generation=generation+1",
    ] {
        let db = Database::new();
        let store = db.store();
        let owner = store.acquire_runtime_lease("runtime".into(), "writer".into(), 3600).unwrap();
        let receipt = store.prepare_gh_write(intent("comment_create")).unwrap();
        let first = store.claim_github_write(owner.clone()).unwrap().unwrap();
        db.connect().execute(change, []).unwrap();
        invalid_data(
            &store.claim_github_write(owner).unwrap_err(),
            "outbox runtime owner expired or replaced",
        );
        invalid_data(
            &store.finish_github_write(first, "applied", None, None, None).unwrap_err(),
            "outbox runtime owner expired or replaced",
        );
        assert_eq!(
            store.gh_write_receipt(receipt.request_id).unwrap().unwrap().lifecycle,
            "sending"
        );
    }
}

#[test]
fn legacy_cli_pending_uncertain_sending_all_get_one_execution_obligation() {
    let db = Database::new();
    let mut connection = open_read_write(&db.path).unwrap();
    configure_connection(&connection).unwrap();
    for migration in MIGRATIONS.iter().filter(|migration| migration.version <= 8) {
        apply_one(&mut connection, migration).unwrap();
    }
    for state in ["pending", "uncertain", "sending"] {
        connection.execute("INSERT INTO write_intents(intent_id,operation,request_digest,lifecycle,created_at,updated_at,request_key,repository,target,profile_id,role,payload) VALUES (?1,'comment_create',?2,?1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',?1,'owner/repository','owner/repository#1','profile','Issue Agent','body')", params![state,"a".repeat(64)]).unwrap();
    }
    drop(connection);
    let store = StoreActor::start(db.path.clone(), db.root.join("backups")).unwrap();
    let migrated = store.apply().unwrap();
    assert_eq!(migrated.applied.first(), Some(&9));
    assert!(migrated.backup.is_some());
    let owner = store.acquire_runtime_lease("runtime".into(), "writer".into(), 3600).unwrap();
    let mut seen = std::collections::BTreeMap::new();
    for _ in 0..3 {
        let write = store.claim_github_write(owner.clone()).unwrap().unwrap();
        seen.insert(write.intent_id.clone(), write.lifecycle.clone());
        store.finish_github_write(write, "applied", None, None, None).unwrap();
    }
    assert_eq!(
        seen,
        std::collections::BTreeMap::from([
            ("pending".into(), "pending".into()),
            ("uncertain".into(), "uncertain".into()),
            ("sending".into(), "uncertain".into())
        ])
    );
    assert!(store.claim_github_write(owner).unwrap().is_none());
    let connection = db.connect();
    assert_eq!(
        connection.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0)).unwrap(),
        "ok"
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn ensure_plan_and_progress_freeze_and_resume_without_replanning() {
    let db = Database::new();
    let store = db.store();
    let write = intent("pr_ensure");
    let receipt = store.prepare_gh_write(write.clone()).unwrap();
    let mut plan = NewImplementationRequest {
        root_intent_id: None,
        write,
        comment_database_id: 7,
        comment_node_id: "comment".into(),
        issue_node_id: "issue".into(),
        issue_number: 1,
        issue_title: "title".into(),
        base_ref: "main".into(),
        head_ref: "head".into(),
        pr_profile_id: "profile".into(),
    };
    let first = store.prepare_implementation_request(plan.clone()).unwrap();
    drop(store);
    let store = db.store();
    plan.base_ref = "changed".into();
    plan.head_ref = "changed".into();
    plan.pr_profile_id = "changed".into();
    let frozen = store.prepare_implementation_request(plan).unwrap();
    assert_eq!(frozen.base_ref, "main");
    assert_eq!(frozen.head_ref, "head");
    assert_eq!(frozen.pr_profile_id, "profile");
    assert_eq!(first.bootstrap_authored_at, frozen.bootstrap_authored_at);
    for stage in ["head_ready", "pull_request_ready", "associated", "activation_pending"] {
        store
            .record_implementation_progress(
                receipt.intent_id.clone(),
                ImplementationProgress {
                    stage,
                    bootstrap_commit_sha: None,
                    pull_request_database_id: Some(88),
                    pull_request_node_id: Some("pr".into()),
                    pull_request_number: Some(9),
                },
            )
            .unwrap();
        let resumed =
            store.implementation_request_receipt(receipt.intent_id.clone()).unwrap().unwrap();
        assert_eq!(resumed.stage, stage);
        assert_eq!(resumed.pull_request_number, Some(9));
    }
    invalid_data(
        &store
            .record_implementation_progress(
                receipt.intent_id,
                ImplementationProgress {
                    stage: "planned",
                    bootstrap_commit_sha: None,
                    pull_request_database_id: None,
                    pull_request_node_id: None,
                    pull_request_number: None,
                },
            )
            .unwrap_err(),
        "cannot move backward from activation_pending to planned",
    );
}
