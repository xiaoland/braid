// Support shims retain production Result signatures.
#![allow(dead_code, clippy::unnecessary_wraps)]
#[path = "../src/telemetry/github.rs"]
pub mod github_metrics;
mod telemetry {
    pub use super::github_metrics as github;
}
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    fs,
    path::{Path, PathBuf},
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
#[derive(Debug, thiserror::Error)]
enum StoreError {
    #[error("{0}")]
    InvalidData(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}
#[derive(Clone, Debug)]
struct RuntimeLease {
    scope: String,
    generation: u64,
    owner_id: String,
}
#[derive(Clone, Debug)]
struct PendingGitHubWrite {
    intent_id: String,
    repository: String,
    target_kind: String,
    target_database_id: String,
    operation: String,
    content: String,
    remote_database_id: Option<String>,
    lifecycle: String,
    created_at: String,
    attempt: i64,
    owner: RuntimeLease,
    retry_after: u64,
    recovery_until: String,
}
fn now_rfc3339() -> String {
    OffsetDateTime::now_utc().format(&Rfc3339).unwrap()
}
fn deadline_rfc3339(seconds: u64) -> Result<String, StoreError> {
    Ok((OffsetDateTime::now_utc() + time::Duration::seconds(i64::try_from(seconds).unwrap()))
        .format(&Rfc3339)
        .unwrap())
}
fn open_read_only(path: &Path) -> Result<Connection, StoreError> {
    Ok(Connection::open(path)?)
}
fn open_read_write(path: &Path) -> Result<Connection, StoreError> {
    open_read_only(path)
}
fn configure_connection(connection: &Connection) -> Result<(), StoreError> {
    Ok(connection.execute_batch("PRAGMA foreign_keys=ON;")?)
}
fn require_current_schema(_: &Path) -> Result<(), StoreError> {
    Ok(())
}
fn sqlite_u64(value: u64, _: &str) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|e| StoreError::InvalidData(e.to_string()))
}
fn sqlite_i64_to_u64(value: i64, _: &str) -> rusqlite::Result<u64> {
    Ok(u64::try_from(value).unwrap())
}
// Operational-status/reaction projections are outside this deep-module harness.
mod reaction_feedback {
    use super::StoreError;
    use rusqlite::Transaction;

    pub(super) fn project(_: &Transaction<'_>) -> Result<(), StoreError> {
        Ok(())
    }

    pub(super) fn rocket_clear_requested_at(
        _: &Transaction<'_>,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<Option<String>, StoreError> {
        Ok(None)
    }

    pub(super) fn enqueue_rocket_removal(
        _: &Transaction<'_>,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}
fn settle_operational_status(
    _: &Connection,
    _: &str,
    _: &str,
    _: Option<&str>,
    _: Option<&str>,
    _: &str,
) -> Result<(), StoreError> {
    Ok(())
}
// Handoff scheduling has production actor/HTTP coverage in writer::tests;
// this harness only exercises publication ledger/execution mechanics.
#[derive(Clone, Copy)]
struct SchedulerPolicy {
    quiet_seconds: u64,
    event_threshold: u32,
}
mod handoff_inputs {
    use super::{SchedulerPolicy, StoreError};

    pub(super) struct NewHandoffInput<'a> {
        pub(super) source_repository: &'a str,
        pub(super) source_number: i64,
        pub(super) target_work_item: &'a str,
        pub(super) target_assignment: &'a str,
        pub(super) target_generation: i64,
    }

    pub(super) fn accept(
        _: &rusqlite::Transaction<'_>,
        _: NewHandoffInput<'_>,
        _: SchedulerPolicy,
    ) -> Result<String, StoreError> {
        unreachable!("handoff is tested through the production StoreActor")
    }
}
#[path = "../src/store/gh_writes.rs"]
mod gh_writes;
#[path = "../src/store/handoff.rs"]
mod handoff;
mod operational_roles {
    pub(super) fn declare(
        _: &rusqlite::Transaction<'_>,
        _: &str,
        _: &str,
        _: i64,
        body: &str,
    ) -> Result<String, super::StoreError> {
        Ok(body.to_owned())
    }
}
#[path = "../src/store/operational_status.rs"]
mod operational_status;
#[path = "../src/store/publication_source.rs"]
mod publication_source;
#[path = "../src/store/write_execution.rs"]
mod write_execution;
use gh_writes::*;
use write_execution::*;
struct Database(PathBuf);
impl Database {
    fn new(migrate: bool) -> Self {
        let path =
            std::env::temp_dir().join(format!("braid-publication-{}.sqlite", uuid::Uuid::now_v7()));
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(include_str!("../migrations/0001_initial.sql")).unwrap();
        connection.execute_batch(include_str!("../migrations/0002_event_kinds.sql")).unwrap();
        if migrate {
            connection
                .execute_batch(include_str!("../migrations/0009_durable_publication.sql"))
                .unwrap();
            connection
                .execute_batch(include_str!("../migrations/0011_collaboration_inputs.sql"))
                .unwrap();
            connection
                .execute_batch(include_str!("../migrations/0012_implementation_delivery.sql"))
                .unwrap();
            connection
                .execute_batch(include_str!(
                    "../migrations/0013_operational_status_convergence.sql"
                ))
                .unwrap();
            connection
                .execute_batch(include_str!("../migrations/0022_operational_status_facts.sql"))
                .unwrap();
        }
        Self(path)
    }
    fn connect(&self) -> Connection {
        Connection::open(&self.0).unwrap()
    }
    fn owner(&self) -> RuntimeLease {
        self.connect()
            .execute(
                "INSERT OR REPLACE INTO owner_leases VALUES ('runtime',1,'owner',?1)",
                [deadline_rfc3339(3600).unwrap()],
            )
            .unwrap();
        RuntimeLease { scope: "runtime".into(), generation: 1, owner_id: "owner".into() }
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
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
#[test]
fn offline_acceptance_is_atomic_idempotent_and_publicly_queryable() {
    let db = Database::new(true);
    let write = intent("comment_create");
    let first = prepare_gh_write(&db.0, &write).unwrap();
    assert!(first.request_id.starts_with("request-"));
    assert_ne!(first.request_id, first.intent_id);
    assert_eq!(first.request_id, prepare_gh_write(&db.0, &write).unwrap().request_id);
    assert_eq!(
        gh_write_receipt(&db.0, &first.request_id).unwrap().unwrap().intent_id,
        first.intent_id
    );
    let mut conflict = write;
    conflict.request_digest = "b".repeat(64);
    assert!(prepare_gh_write(&db.0, &conflict).is_err());
    let count: i64 = db
        .connect()
        .query_row("SELECT COUNT(*) FROM github_write_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn preflight_limit_survives_exit_and_reset_deadline_then_restart_recovers_attempt_fence() {
    let db = Database::new(true);
    let owner = db.owner();
    prepare_gh_write(&db.0, &intent("comment_create")).unwrap();
    let mut first = claim_github_write(&db.0, &owner).unwrap().unwrap();
    first.retry_after = 3600;
    finish_github_write(&db.0, &first, "pending", None, None, Some("limit before mutation"))
        .unwrap();
    assert!(claim_github_write(&db.0, &owner).unwrap().is_none());
    db.connect()
        .execute("UPDATE github_write_outbox SET next_attempt_at='2000-01-01T00:00:00Z'", [])
        .unwrap();
    let second = claim_github_write(&db.0, &owner).unwrap().unwrap();
    recover_writes(&db.0).unwrap();
    let third = claim_github_write(&db.0, &owner).unwrap().unwrap();
    assert_eq!(third.lifecycle, "uncertain");
    assert!(finish_github_write(&db.0, &second, "applied", None, None, None).is_err());
    finish_github_write(&db.0, &third, "applied", Some("123"), None, None).unwrap();
    assert!(claim_github_write(&db.0, &owner).unwrap().is_none());
}
#[test]
fn expired_or_replaced_runtime_cannot_claim_or_settle() {
    let db = Database::new(true);
    let owner = db.owner();
    prepare_gh_write(&db.0, &intent("comment_create")).unwrap();
    let first = claim_github_write(&db.0, &owner).unwrap().unwrap();
    db.connect().execute("UPDATE owner_leases SET owner_id='new',generation=2", []).unwrap();
    assert!(claim_github_write(&db.0, &owner).is_err());
    assert!(finish_github_write(&db.0, &first, "applied", None, None, None).is_err());
}
#[test]
fn legacy_cli_pending_uncertain_sending_all_get_one_execution_obligation() {
    let db = Database::new(false);
    for state in ["pending", "uncertain", "sending"] {
        db.connect().execute("INSERT INTO write_intents(intent_id,operation,request_digest,lifecycle,created_at,updated_at,request_key,repository,target,profile_id,role,payload) VALUES (?1,'comment_create',?2,?1,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z',?1,'owner/repository','owner/repository#1','profile','Issue Agent','body')",params![state,"a".repeat(64)]).unwrap();
    }
    db.connect().execute_batch(include_str!("../migrations/0009_durable_publication.sql")).unwrap();
    db.connect()
        .execute_batch(include_str!("../migrations/0011_collaboration_inputs.sql"))
        .unwrap();
    db.connect()
        .execute_batch(include_str!("../migrations/0012_implementation_delivery.sql"))
        .unwrap();
    db.connect()
        .execute_batch(include_str!("../migrations/0013_operational_status_convergence.sql"))
        .unwrap();
    db.connect()
        .execute_batch(include_str!("../migrations/0022_operational_status_facts.sql"))
        .unwrap();
    let owner = db.owner();
    for _ in 0..3 {
        let write = claim_github_write(&db.0, &owner).unwrap().unwrap();
        assert_ne!(write.lifecycle, "sending");
        finish_github_write(&db.0, &write, "applied", None, None, None).unwrap();
    }
    assert!(claim_github_write(&db.0, &owner).unwrap().is_none());
}
#[test]
fn ensure_plan_and_progress_freeze_and_resume_without_replanning() {
    let db = Database::new(true);
    let write = intent("pr_ensure");
    let receipt = prepare_gh_write(&db.0, &write).unwrap();
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
    let first = prepare_implementation_request(&db.0, &plan).unwrap();
    plan.base_ref = "changed".into();
    plan.head_ref = "changed".into();
    plan.pr_profile_id = "changed".into();
    let frozen = prepare_implementation_request(&db.0, &plan).unwrap();
    assert_eq!(frozen.base_ref, "main");
    assert_eq!(frozen.head_ref, "head");
    assert_eq!(frozen.pr_profile_id, "profile");
    assert_eq!(first.bootstrap_authored_at, frozen.bootstrap_authored_at);
    for stage in ["head_ready", "pull_request_ready", "associated", "activation_pending"] {
        record_implementation_progress(
            &db.0,
            &receipt.intent_id,
            &ImplementationProgress {
                stage,
                bootstrap_commit_sha: None,
                pull_request_database_id: Some(88),
                pull_request_node_id: Some("pr".into()),
                pull_request_number: Some(9),
            },
        )
        .unwrap();
        let resumed = implementation_request_receipt(&db.0, &receipt.intent_id).unwrap().unwrap();
        assert_eq!(resumed.stage, stage);
        assert_eq!(resumed.pull_request_number, Some(9));
    }
    assert!(
        record_implementation_progress(
            &db.0,
            &receipt.intent_id,
            &ImplementationProgress {
                stage: "planned",
                bootstrap_commit_sha: None,
                pull_request_database_id: None,
                pull_request_node_id: None,
                pull_request_number: None
            }
        )
        .is_err()
    );
}

mod config {
    #[derive(Clone)]
    pub struct GitHubConfig {
        pub app_id: u64,
        pub api_version: String,
        pub private_key_file: std::path::PathBuf,
        pub projects_v2_enabled: bool,
    }
}
#[path = "../src/github.rs"]
mod github;
use github::{GitHubClient, GitHubError};
struct Config;
fn check_source(_: &Config, _: &GhWriteReceipt) -> Result<(), GitHubError> {
    Ok(())
}
// Source/target authorization is covered by the production writer actor tests.
mod edit {
    use super::{Config, GhWriteReceipt, GitHubClient, GitHubError};
    #[allow(clippy::unused_async)] // Match the production authorization boundary.
    pub async fn authorize(
        _: &Config,
        _: &GitHubClient,
        _: &GhWriteReceipt,
        _: bool,
    ) -> Result<(), GitHubError> {
        Ok(())
    }
}
mod helpers {
    use crate::github::GitHubError;
    #[derive(Debug)]
    pub struct RemoteWrite {
        pub database_id: String,
        pub node_id: String,
    }
    #[derive(Debug)]
    pub enum WriteFailure {
        GitHub(GitHubError),
        Ambiguous(String),
    }
    impl From<GitHubError> for WriteFailure {
        fn from(error: GitHubError) -> Self {
            Self::GitHub(error)
        }
    }
    pub type WriteResult = Result<RemoteWrite, WriteFailure>;
}
#[path = "../src/writer/comment.rs"]
mod comment;

async fn comment_server(
    comments: serde_json::Value,
    permission: bool,
    target_status: u16,
) -> (
    github::GitHubClient,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    use axum::{Json, Router, routing::get};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let writes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&writes);
    let router = Router::new().route(
        "/repos/owner/repository/issues/1/comments",
        get(move || {
            let comments = comments.clone();
            async move { Json(comments) }
        })
        .post(move || {
            count.fetch_add(1, Ordering::SeqCst);
            async { Json(serde_json::json!({"id":99,"node_id":"comment-99"})) }
        }),
    );
    let router = router.route("/repos/owner/repository/issues/1", get(move || async move {
        use axum::response::IntoResponse as _;
        if target_status != 200 { return (axum::http::StatusCode::from_u16(target_status).unwrap(), Json(serde_json::json!({"message":"target unavailable"}))).into_response(); }
        Json(serde_json::json!({"id":1,"node_id":"issue","number":1,"title":"Issue","html_url":"https://github.com/owner/repository/issues/1","state":"open","pull_request":null})).into_response()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let http = octocrab::Octocrab::builder()
        .base_uri(format!("http://{address}"))
        .unwrap()
        .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
        .build()
        .unwrap();
    let identity = github::GitHubIdentity {
        app_id: 1,
        app_slug: "braid".into(),
        installation_id: 1,
        repository: "owner/repository".into(),
        repository_node_id: "repo".into(),
        actor_node_id: "app".into(),
        actor_login: "braid[bot]".into(),
        token_expires_at: String::new(),
        permissions: if permission {
            std::collections::BTreeMap::from([("issues".into(), "write".into())])
        } else {
            std::collections::BTreeMap::new()
        },
    };
    let github = github::GitHubClient::publication_test_client(
        http,
        identity,
        config::GitHubConfig {
            app_id: 1,
            api_version: "2022-11-28".into(),
            private_key_file: PathBuf::new(),
            projects_v2_enabled: false,
        },
    );
    (github, writes, task)
}
fn remote_comment(id: u64) -> serde_json::Value {
    serde_json::json!({"id":id,"node_id":format!("comment-{id}"),"url":"https://api.github.com/repos/owner/repository/issues/comments/1","html_url":"https://github.com/owner/repository/issues/1#issuecomment-1","issue_url":"https://api.github.com/repos/owner/repository/issues/1","body":"raw body","created_at":now_rfc3339(),"updated_at":now_rfc3339(),"user":{"login":"braid[bot]","id":1,"node_id":"app","avatar_url":"https://example.invalid/avatar","url":"https://api.github.com/users/braid","html_url":"https://github.com/apps/braid","type":"Bot","site_admin":false}})
}
#[tokio::test]
async fn successful_unacknowledged_comment_is_recovered_without_a_second_mutation() {
    let db = Database::new(true);
    let owner = db.owner();
    let mut receipt = prepare_gh_write(&db.0, &intent("comment_create")).unwrap();
    claim_github_write(&db.0, &owner).unwrap().unwrap();
    recover_writes(&db.0).unwrap();
    let write = claim_github_write(&db.0, &owner).unwrap().unwrap();
    receipt.attempts = 1;
    let (github, writes, task) =
        comment_server(serde_json::json!([remote_comment(77)]), false, 200).await;
    let recovered =
        comment::converge_comment(&Config, &github, 1, &receipt, &mut true).await.unwrap();
    assert_eq!(recovered.database_id, "77");
    assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    finish_github_write(
        &db.0,
        &write,
        "applied",
        Some(&recovered.database_id),
        Some(&recovered.node_id),
        None,
    )
    .unwrap();
    task.abort();
}
#[tokio::test]
async fn ambiguous_comment_recovery_never_mutates() {
    let db = Database::new(true);
    let mut receipt = prepare_gh_write(&db.0, &intent("comment_create")).unwrap();
    receipt.attempts = 1;
    let (github, writes, task) =
        comment_server(serde_json::json!([remote_comment(77), remote_comment(78)]), false, 200)
            .await;
    let result = comment::converge_comment(&Config, &github, 1, &receipt, &mut true).await;
    assert!(matches!(result, Err(helpers::WriteFailure::Ambiguous(_))));
    assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    task.abort();
}

#[test]
fn cli_exits_offline_with_queryable_receipt_and_no_remote_credentials() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("braid-cli-publication-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let example = include_str!("../config.example.toml");
    fs::write(&path, example.replace("/absolute/path/to/instance", dir.to_str().unwrap())).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_braid"))
            .args(args)
            .arg("--config")
            .arg(&path)
            .env_remove("BRAID_AGENT")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output
    };
    run(&["migrate", "apply"]);
    let snapshot: github::BodySnapshot = serde_json::from_value(serde_json::json!({"id":1,"node_id":"item","title":"Title","body":"Closes #2\n\nImplementation request: durable-marker","state":"open","updated_at":"version-1","pull_request":{}})).unwrap();
    let snapshot_file = dir.join("snapshot.json");
    fs::write(&snapshot_file,serde_json::to_string(&serde_json::json!({"target":"owner/repository#1","digest":snapshot.digest(),"snapshot":snapshot})).unwrap()).unwrap();
    let body_file = dir.join("body.md");
    fs::write(&body_file, "Implementation scope and verification").unwrap();
    let edited = run(&[
        "gh",
        "pr",
        "edit",
        "owner/repository#1",
        "--operator",
        "--profile",
        "pr-codex",
        "--snapshot",
        snapshot_file.to_str().unwrap(),
        "--body-file",
        body_file.to_str().unwrap(),
        "--request-id",
        "offline-body",
        "--json",
    ]);
    let edited: serde_json::Value = serde_json::from_slice(&edited.stdout).unwrap();
    assert_eq!(edited["state"], "queued");
    assert_eq!(edited["operation"], "body_edit");
    assert_eq!(edited["role"], "Operator");
    let status = run(&["gh", "status", edited["request_id"].as_str().unwrap(), "--json"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["state"],
        "queued"
    );
    let output = run(&[
        "gh",
        "comment",
        "create",
        "--operator",
        "owner/repository#1",
        "--profile",
        "issue-codex",
        "--request-id",
        "offline",
        "--body",
        "Offline update",
        "--json",
    ]);
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "queued");
    assert!(receipt.get("intent_id").is_none());
    let id = receipt["request_id"].as_str().unwrap();
    let output = run(&["gh", "status", id, "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["request_id"], id);
    assert_eq!(status["state"], "queued");
    let output = run(&[
        "gh",
        "comment",
        "create",
        "--operator",
        "owner/repository#1",
        "--profile",
        "issue-codex",
        "--request-id",
        "offline",
        "--body",
        "Offline update",
        "--json",
    ]);
    let duplicate: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(duplicate["request_id"], id);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn gh_read_runs_on_existing_runtime_without_nested_runtime_panic() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("braid-cli-read-{}-", uuid::Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let example = include_str!("../config.example.toml");
    fs::write(&path, example.replace("/absolute/path/to/instance", dir.to_str().unwrap())).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_braid"))
        .args(["gh", "pr", "read", "owner/repository#1"])
        .arg("--output")
        .arg(dir.join("snapshot.json"))
        .arg("--config")
        .arg(&path)
        .env_remove("BRAID_AGENT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Cannot start a runtime from within a runtime"),
        "nested runtime panic: {stderr}"
    );
    assert!(!output.status.success(), "read must fail without GitHub credentials");
    assert!(output.status.code() != Some(101), "cli panicked: {stderr}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn completed_turn_keeps_obligation_but_invalidation_and_generation_replace_fence_it() {
    let db = Database::new(true);
    let connection = db.connect();
    connection.execute_batch("INSERT INTO repositories VALUES ('repo','owner/repository',1,'now'); INSERT INTO work_items VALUES ('item','repo','issue',1,'open','context','now'); INSERT INTO profiles VALUES ('profile',1,'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','pi','issue'); INSERT INTO assignments VALUES ('assignment','item',1,'active','now',NULL); INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle) VALUES ('agent','assignment','profile',1,'issue_agent','idle'); INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at) VALUES ('session','agent','pi','provider','context','instructions','idle','now'); INSERT INTO turns(turn_id,session_id,context_revision,trigger_kind,lifecycle) VALUES ('turn','session','context','wake','completed');").unwrap();
    let source = publication_source::PublicationSource {
        agent_id: "agent".into(),
        assignment_id: "assignment".into(),
        generation: 1,
        profile_id: "profile".into(),
        role: "issue_agent".into(),
        profile_revision: 1,
        session_id: "session".into(),
        turn_id: "turn".into(),
        context_revision: "context".into(),
    };
    connection.execute("INSERT INTO worktrees(worktree_id,agent_id,path,repository_node_id,lifecycle,observed_at) VALUES ('tree','agent','/source','repo','active','now')", []).unwrap();
    assert!(publication_source::check(&db.0, &source).is_ok());
    connection.execute("INSERT INTO events(event_id,work_item_node_id,origin,reference,lifecycle,observed_at,kind) VALUES ('invalidate','item','human','changed','pending','now','invalidate')", []).unwrap();
    assert!(publication_source::check(&db.0, &source).is_err());
    connection.execute("DELETE FROM events WHERE event_id='invalidate'", []).unwrap();
    connection.execute("UPDATE provider_sessions SET lifecycle='reset_pending'", []).unwrap();
    assert!(publication_source::check(&db.0, &source).is_err());
    connection.execute("UPDATE provider_sessions SET lifecycle='idle'", []).unwrap();
    connection.execute("UPDATE assignments SET lifecycle='retired'", []).unwrap();
    connection
        .execute("INSERT INTO assignments VALUES ('replacement','item',2,'active','now',NULL)", [])
        .unwrap();
    assert!(publication_source::check(&db.0, &source).is_err());
}

#[tokio::test]
async fn permission_and_target_failures_do_not_mutate_and_preflight_limit_stays_pending() {
    for (permission, status) in [(false, 200), (true, 404), (true, 429)] {
        let db = Database::new(true);
        let owner = db.owner();
        let receipt = prepare_gh_write(&db.0, &intent("comment_create")).unwrap();
        let write = claim_github_write(&db.0, &owner).unwrap().unwrap();
        let (github, writes, task) =
            comment_server(serde_json::json!([]), permission, status).await;
        let mut mutation = false;
        let result = comment::converge_comment(&Config, &github, 1, &receipt, &mut mutation).await;
        assert!(result.is_err());
        assert!(!mutation);
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        let helpers::WriteFailure::GitHub(error) = result.unwrap_err() else {
            panic!("expected target/permission error")
        };
        let state = if error.is_unavailable() { "pending" } else { "rejected" };
        assert_eq!(state, if status == 429 { "pending" } else { "rejected" });
        finish_github_write(&db.0, &write, state, None, None, Some(&error.to_string())).unwrap();
        assert!(claim_github_write(&db.0, &owner).unwrap().is_none());
        task.abort();
    }
}
#[tokio::test]
async fn accepted_obligation_converges_from_reopened_database_without_cli_execution() {
    let db = Database::new(true);
    let receipt = prepare_gh_write(&db.0, &intent("comment_create")).unwrap();
    // No connection or execution owner survives acceptance. A new runtime owns delivery.
    let owner = db.owner();
    recover_writes(&db.0).unwrap();
    let write = claim_github_write(&db.0, &owner).unwrap().unwrap();
    let (github, writes, task) = comment_server(serde_json::json!([]), true, 200).await;
    let remote =
        comment::converge_comment(&Config, &github, 1, &receipt, &mut false).await.unwrap();
    finish_github_write(
        &db.0,
        &write,
        "applied",
        Some(&remote.database_id),
        Some(&remote.node_id),
        None,
    )
    .unwrap();
    assert_eq!(gh_write_receipt(&db.0, &receipt.request_id).unwrap().unwrap().lifecycle, "applied");
    assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 1);
    recover_writes(&db.0).unwrap();
    assert!(claim_github_write(&db.0, &owner).unwrap().is_none());
    task.abort();
}
