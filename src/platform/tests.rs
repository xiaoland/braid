//! Public object protocol over a real Git repository and reopened platform database.
use super::{PlatformConfig, router};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::path::PathBuf;

struct Fixture {
    root: PathBuf,
    config: PlatformConfig,
    client: reqwest::Client,
    server: Option<tokio::task::JoinHandle<()>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("braid-platform-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let git = git2::Repository::init(&source).unwrap();
        commit(&git, "refs/heads/main", "base\n", &[]);
        git.set_head("refs/heads/main").unwrap();
        let mut fixture = Self {
            config: PlatformConfig {
                schema_version: 1,
                repository: "contract-space".into(),
                source: source.canonicalize().unwrap(),
                default_branch: "main".into(),
                database: root.join("platform.sqlite3"),
                backups: root.join("backups"),
                listen: String::new(),
            },
            root,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
            server: None,
            stop: None,
        };
        fixture.start().await;
        fixture
    }

    async fn start(&mut self) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.config.listen = listener.local_addr().unwrap().to_string();
        let app = router(&self.config).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        self.stop = Some(stop);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        }));
    }

    async fn restart(&mut self) {
        self.shutdown().await;
        self.start().await;
    }

    async fn shutdown(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.server.take().unwrap().await.unwrap();
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.config.listen)
    }

    async fn get(&self, path: &str) -> Value {
        self.client
            .get(self.url(path))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn post(&self, path: &str, body: &Value) -> (StatusCode, Value) {
        let response = self.client.post(self.url(path)).json(body).send().await.unwrap();
        (response.status(), response.json().await.unwrap())
    }

    async fn apply(&self, id: &str, revision: Option<u64>, command: Value) -> Value {
        let (status, receipt) = self
            .post(
                "/api/control/commands",
                &json!({
                    "requestId": id, "expectedRevision": revision, "command": command,
                }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{receipt}");
        assert_eq!(receipt["state"], "applied", "{receipt}");
        receipt
    }

    async fn create_issue(&self) {
        let receipt = self
            .apply(
                "create",
                None,
                json!({"type":"createIssue", "title":"Design", "body":"original"}),
            )
            .await;
        assert_eq!(receipt["result"]["number"], 1);
        assert_eq!(receipt["result"]["revision"], 1);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn commit(git: &git2::Repository, reference: &str, body: &str, parents: &[git2::Oid]) -> git2::Oid {
    let blob = git.blob(body.as_bytes()).unwrap();
    let mut tree = git.treebuilder(None).unwrap();
    tree.insert("README.md", blob, 0o100_644).unwrap();
    let tree = git.find_tree(tree.write().unwrap()).unwrap();
    let signature = git2::Signature::now("Contract", "contract@example.invalid").unwrap();
    let parents = parents.iter().map(|id| git.find_commit(*id).unwrap()).collect::<Vec<_>>();
    let parents = parents.iter().collect::<Vec<_>>();
    git.commit(Some(reference), &signature, &signature, "candidate", &tree, &parents).unwrap()
}

#[tokio::test]
async fn command_receipts_and_revision_conflicts_survive_restart_without_duplicate_events() {
    let mut fixture = Fixture::new().await;
    fixture.create_issue().await;
    let original = fixture.get("/api/control/receipts/create").await;
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    let replay = fixture
        .apply("create", None, json!({"type":"createIssue", "title":"Design", "body":"original"}))
        .await;
    assert_eq!(replay, original);
    assert_eq!(fixture.get("/api/control/items").await.as_array().unwrap().len(), 1);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);

    let (status, error) = fixture.post("/api/control/commands", &json!({
        "requestId":"create", "command":{"type":"createIssue", "title":"Different", "body":""},
    })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["error"]["code"], "conflict");
    assert!(error["error"]["message"].as_str().unwrap().contains("requestId was already used"));
    assert_eq!(fixture.get("/api/control/receipts/create").await, original);

    fixture.apply("edit", Some(1), json!({"type":"edit", "number":1, "body":"accepted"})).await;
    let events = fixture.get("/api/platform/events?after=0").await;
    let stale = json!({"requestId":"stale", "expectedRevision":1,
        "command":{"type":"edit", "number":1, "body":"must not overwrite"}});
    let (status, rejection) = fixture.post("/api/control/commands", &stale).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejection["state"], "rejected");
    assert_eq!(rejection["error"]["code"], "conflict");
    assert_eq!(rejection["error"]["message"], "object #1 revision is 2, expected 1");
    fixture.restart().await;
    assert_eq!(fixture.get("/api/control/receipts/stale").await, rejection);
    let item = fixture.get("/api/control/items/1").await;
    assert_eq!(item["item"]["body"], "accepted");
    assert_eq!(item["item"]["revision"], 2);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    fixture.shutdown().await;
}

#[tokio::test]
async fn revoked_writer_cannot_write_with_the_current_object_revision_after_restart() {
    let mut fixture = Fixture::new().await;
    fixture.create_issue().await;
    fixture.apply("assign", Some(1), json!({"type":"assign", "number":1, "actor":"worker"})).await;
    let grant_request = json!({"requestId":"grant", "clientId":"harness", "itemNumber":1,
        "actor":"worker", "expectedEpoch":0});
    let (status, grant) = fixture.post("/api/platform/grants", &grant_request).await;
    assert_eq!(status, StatusCode::OK, "{grant}");
    assert_eq!(grant["epoch"], 1);
    let capability =
        json!({"grantId":grant["grantId"], "epoch":grant["epoch"], "token":grant["token"]});
    let write = json!({"requestId":"write", "expectedRevision":2, "actor":"worker", "writer":capability,
        "command":{"type":"comment", "number":1, "body":"accepted", "replyTo":null}});
    let (status, accepted) = fixture.post("/api/control/commands", &write).await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["state"], "applied");
    let (status, revoked) = fixture.post("/api/platform/grants/revoke", &json!({
        "requestId":"revoke", "clientId":"harness", "itemNumber":1, "actor":"worker", "expectedEpoch":1,
    })).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked, json!({"epoch":2,"lifecycle":"revoked"}));
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    let item = fixture.get("/api/control/items/1").await;
    assert_eq!(item["item"]["state"], "open");
    assert_eq!(item["item"]["desired_profile"], "worker");
    assert_eq!(item["item"]["revision"], 3);
    let (status, rejected) = fixture.post("/api/control/commands", &json!({
        "requestId":"late", "expectedRevision":3, "actor":"worker", "writer":capability,
        "command":{"type":"comment", "number":1, "body":"must not be written", "replyTo":null},
    })).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["error"]["code"], "rejected");
    assert!(
        rejected["error"]["message"].as_str().unwrap().contains("writer grant is stale, revoked")
    );
    assert_eq!(fixture.get("/api/control/items/1").await, item);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    // A completed old command remains a receipt; replay is not a new late mutation.
    let (status, replay) = fixture.post("/api/control/commands", &write).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, accepted);
    fixture.shutdown().await;
}

#[tokio::test]
async fn published_candidate_diff_and_merge_use_the_frozen_commit_after_branch_moves() {
    let mut fixture = Fixture::new().await;
    fixture.create_issue().await;
    let git = git2::Repository::open(&fixture.config.source).unwrap();
    let base = git.refname_to_id("refs/heads/main").unwrap();
    let first = commit(&git, "refs/heads/feature", "first candidate\n", &[base]);
    fixture
        .apply(
            "pr",
            None,
            json!({"type":"createPR", "title":"Implementation", "body":"",
        "head":"feature", "base":"main", "issue":1, "issueRevision":1}),
        )
        .await;
    let published = commit(&git, "refs/heads/feature", "published candidate\n", &[first]);
    fixture
        .apply(
            "publish",
            Some(1),
            json!({"type":"publish", "number":2,
        "candidate":{"type":"commit", "commit":published.to_string()}}),
        )
        .await;
    commit(&git, "refs/heads/feature", "later unpublished work\n", &[published]);
    git.set_head_detached(base).unwrap();
    fixture.restart().await;
    let diff = fixture.get("/api/control/items/2/diff").await;
    assert_eq!(diff["number"], 2);
    assert_eq!(diff["revision"], 2);
    assert_eq!(diff["headCommit"], published.to_string());
    assert_eq!(diff["baseCommit"], base.to_string());
    assert!(diff["diff"].as_str().unwrap().contains("+published candidate"));
    assert!(!diff["diff"].as_str().unwrap().contains("later unpublished work"));
    let receipt = fixture.apply("merge", Some(2), json!({"type":"merge", "number":2})).await;
    let merged = git.refname_to_id("refs/heads/main").unwrap();
    assert_eq!(receipt["result"]["merge_commit"], merged.to_string());
    let commit = git.find_commit(merged).unwrap();
    assert_eq!(commit.parent_count(), 2);
    assert_eq!(commit.parent_id(0).unwrap(), base);
    assert_eq!(commit.parent_id(1).unwrap(), published);
    let tree = commit.tree().unwrap();
    let blob = git.find_blob(tree.get_name("README.md").unwrap().id()).unwrap();
    assert_eq!(blob.content(), b"published candidate\n");
    assert_eq!(fixture.get("/api/control/items/1").await["item"]["state"], "open");
    assert_eq!(fixture.get("/api/control/items/2").await["item"]["state"], "closed");
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    let replay = fixture.apply("merge", Some(2), json!({"type":"merge", "number":2})).await;
    assert_eq!(replay, receipt);
    assert_eq!(git.refname_to_id("refs/heads/main").unwrap(), merged);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    fixture.shutdown().await;
}

async fn assigned_pr(fixture: &Fixture) -> Value {
    fixture.create_issue().await;
    let git = git2::Repository::open(&fixture.config.source).unwrap();
    let base = git.refname_to_id("refs/heads/main").unwrap();
    commit(&git, "refs/heads/feature", "candidate\n", &[base]);
    git.set_head_detached(base).unwrap();
    fixture
        .apply(
            "pr",
            None,
            json!({"type":"createPR", "title":"Implementation", "body":"",
        "head":"feature", "base":"main", "issue":1, "issueRevision":1}),
        )
        .await;
    fixture
        .apply("assign-pr", Some(1), json!({"type":"assign", "number":2, "actor":"worker"}))
        .await;
    let (status, grant) = fixture
        .post(
            "/api/platform/grants",
            &json!({"requestId":"grant-pr",
        "clientId":"harness", "itemNumber":2, "actor":"worker", "expectedEpoch":0}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    json!({"grantId":grant["grantId"],"epoch":grant["epoch"],"token":grant["token"]})
}

#[tokio::test]
async fn adopted_merge_recovers_its_receipt_after_restart_and_writer_revocation() {
    let mut fixture = Fixture::new().await;
    let capability = assigned_pr(&fixture).await;
    let request = json!({"requestId":"merge-lost", "expectedRevision":2,
        "actor":"worker", "writer":capability, "command":{"type":"merge", "number":2}});
    let db = rusqlite::Connection::open(&fixture.config.database).unwrap();
    // Git CAS succeeds. Failing final receipt storage must roll back the matching
    // SQL effects, while retaining this request's exact prepared Git commit.
    db.execute_batch("CREATE TRIGGER lose_receipt BEFORE UPDATE OF response_json ON control_receipts WHEN NEW.request_id='merge-lost' AND NEW.state!='prepared' BEGIN SELECT RAISE(ABORT, 'injected receipt storage failure'); END;").unwrap();
    let before = fixture.get("/api/platform/events?after=0").await;
    let (status, failure) = fixture.post("/api/control/commands", &request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        failure["error"]["message"].as_str().unwrap().contains("injected receipt storage failure")
    );
    let pending = fixture.get("/api/control/items/2").await;
    assert_eq!(
        pending["item"]["state"], "open",
        "merge completion must commit atomically with its command receipt"
    );
    assert_eq!(pending["item"]["revision"], 2);
    assert_eq!(pending["merge"]["lifecycle"], "prepared");
    let merged = pending["merge"]["merge_commit"].as_str().unwrap().to_owned();
    let prepared = fixture.get("/api/control/receipts/merge-lost").await;
    assert_eq!(prepared["state"], "prepared");
    assert_eq!(prepared["result"]["merge"]["merge_commit"], merged);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, before);
    let git = git2::Repository::open(&fixture.config.source).unwrap();
    assert_eq!(git.refname_to_id("refs/heads/main").unwrap().to_string(), merged);
    db.execute_batch("DROP TRIGGER lose_receipt").unwrap();
    drop(db);
    let (status, revoked) = fixture.post("/api/platform/grants/revoke", &json!({
        "requestId":"revoke-pr", "clientId":"harness", "itemNumber":2, "actor":"worker", "expectedEpoch":1,
    })).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["lifecycle"], "revoked");
    fixture.restart().await;
    // A differently bound payload cannot consume this request's prepared result.
    let mut different = request.clone();
    different["actor"] = json!("other-worker");
    let (status, rejected) = fixture.post("/api/control/commands", &different).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(rejected["error"]["message"].as_str().unwrap().contains("requestId was already used"));
    assert_eq!(fixture.get("/api/control/receipts/merge-lost").await, prepared);
    let (status, recovered) = fixture.post("/api/control/commands", &request).await;
    assert_eq!(status, StatusCode::OK, "{recovered}");
    assert_eq!(recovered["state"], "applied");
    assert_eq!(recovered["result"]["recovered"], true);
    assert_eq!(recovered["result"]["merge_commit"], merged);
    assert_eq!(fixture.get("/api/control/receipts/merge-lost").await, recovered);
    let item = fixture.get("/api/control/items/2").await;
    assert_eq!(item["item"]["state"], "closed");
    assert_eq!(item["item"]["revision"], 3);
    assert_eq!(item["merge"]["lifecycle"], "applied");
    let db = rusqlite::Connection::open(&fixture.config.database).unwrap();
    let origins: Vec<(String, String)> = db
        .prepare("SELECT actor,origin FROM local_events WHERE detail='merged'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(origins, [("worker".into(), "agent".into())]);
    drop(db);
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    assert_eq!(fixture.post("/api/control/commands", &request).await, (StatusCode::OK, recovered));
    assert_eq!(fixture.get("/api/control/items/2").await, item);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    assert_eq!(git.refname_to_id("refs/heads/main").unwrap().to_string(), merged);
    fixture.shutdown().await;
}

#[tokio::test]
async fn prepared_merge_does_not_retry_uncommitted_effects_after_writer_revocation() {
    let mut fixture = Fixture::new().await;
    let capability = assigned_pr(&fixture).await;
    let request = json!({"requestId":"merge-pending", "expectedRevision":2,
        "actor":"worker", "writer":capability, "command":{"type":"merge", "number":2}});
    let db = rusqlite::Connection::open(&fixture.config.database).unwrap();
    db.execute_batch("CREATE TRIGGER fail_intent BEFORE INSERT ON local_merges BEGIN SELECT RAISE(ABORT, 'injected merge intent failure'); END;
        CREATE TRIGGER lose_receipt BEFORE UPDATE OF response_json ON control_receipts WHEN NEW.request_id='merge-pending' BEGIN SELECT RAISE(ABORT, 'injected receipt storage failure'); END;").unwrap();
    let (status, failure) = fixture.post("/api/control/commands", &request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        failure["error"]["message"].as_str().unwrap().contains("injected receipt storage failure")
    );
    let item = fixture.get("/api/control/items/2").await;
    assert!(item["merge"].is_null());
    assert_eq!(item["item"]["state"], "open");
    let git = git2::Repository::open(&fixture.config.source).unwrap();
    let base = git.refname_to_id("refs/heads/main").unwrap();
    db.execute_batch("DROP TRIGGER fail_intent; DROP TRIGGER lose_receipt;").unwrap();
    drop(db);
    let (status, revoked) = fixture.post("/api/platform/grants/revoke", &json!({
        "requestId":"revoke-pr", "clientId":"harness", "itemNumber":2, "actor":"worker", "expectedEpoch":1,
    })).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["lifecycle"], "revoked");
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    let (status, pending) = fixture.post("/api/control/commands", &request).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{pending}");
    assert_eq!(pending["state"], "prepared");
    assert_eq!(fixture.get("/api/control/items/2").await, item);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    assert_eq!(git.refname_to_id("refs/heads/main").unwrap(), base);
    fixture.shutdown().await;
}

#[tokio::test]
async fn a_bound_but_unadopted_merge_cannot_use_retry_to_bypass_revocation() {
    let mut fixture = Fixture::new().await;
    let capability = assigned_pr(&fixture).await;
    let request = json!({"requestId":"merge-locked", "expectedRevision":2,
        "actor":"worker", "writer":capability, "command":{"type":"merge", "number":2}});
    let git = git2::Repository::open(&fixture.config.source).unwrap();
    let base = git.refname_to_id("refs/heads/main").unwrap();
    let lock = git.path().join("refs/heads/main.lock");
    std::fs::write(&lock, b"held by contract test").unwrap();
    let (status, blocked) = fixture.post("/api/control/commands", &request).await;
    assert_eq!(status, StatusCode::CONFLICT, "{blocked}");
    assert_eq!(blocked["state"], "blocked");
    assert!(
        blocked["error"]["message"].as_str().unwrap().contains("base ref compare-and-swap failed")
    );
    assert!(blocked["result"]["merge"]["merge_commit"].is_string());
    assert_eq!(git.refname_to_id("refs/heads/main").unwrap(), base);
    std::fs::remove_file(lock).unwrap();
    let (status, revoked) = fixture.post("/api/platform/grants/revoke", &json!({
        "requestId":"revoke-pr", "clientId":"harness", "itemNumber":2, "actor":"worker", "expectedEpoch":1,
    })).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    let item = fixture.get("/api/control/items/2").await;
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    assert_eq!(
        fixture.post("/api/control/commands", &request).await,
        (StatusCode::CONFLICT, blocked)
    );
    assert_eq!(fixture.get("/api/control/items/2").await, item);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    assert_eq!(git.refname_to_id("refs/heads/main").unwrap(), base);
    fixture.shutdown().await;
}

#[tokio::test]
async fn publishing_and_its_receipt_commit_together_or_leave_the_candidate_unchanged() {
    let mut fixture = Fixture::new().await;
    let capability = assigned_pr(&fixture).await;
    let git = git2::Repository::open(&fixture.config.source).unwrap();
    let previous = git.refname_to_id("refs/heads/feature").unwrap();
    let candidate = commit(
        &git,
        "refs/heads/feature",
        "publication
",
        &[previous],
    );
    let request = json!({"requestId":"publish-lost", "expectedRevision":2, "actor":"worker", "writer":capability,
        "command":{"type":"publish", "number":2, "candidate":{"type":"commit", "commit":candidate.to_string()}}});
    let item = fixture.get("/api/control/items/2").await;
    let events = fixture.get("/api/platform/events?after=0").await;
    let db = rusqlite::Connection::open(&fixture.config.database).unwrap();
    db.execute_batch("CREATE TRIGGER lose_publish_receipt BEFORE UPDATE OF response_json ON control_receipts WHEN NEW.request_id='publish-lost' BEGIN SELECT RAISE(ABORT, 'injected publish receipt failure'); END;").unwrap();
    let (status, failure) = fixture.post("/api/control/commands", &request).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        failure["error"]["message"].as_str().unwrap().contains("injected publish receipt failure")
    );
    assert_eq!(
        fixture.get("/api/control/items/2").await,
        item,
        "publication must commit atomically with its command receipt"
    );
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    db.execute_batch("DROP TRIGGER lose_publish_receipt").unwrap();
    drop(db);
    fixture.restart().await;
    assert_eq!(fixture.get("/api/control/items/2").await, item);
    let mut new_request = request.clone();
    new_request["requestId"] = json!("publish-complete");
    let (status, receipt) = fixture.post("/api/control/commands", &new_request).await;
    assert_eq!(status, StatusCode::OK, "{receipt}");
    assert_eq!(receipt["result"]["head_commit"], candidate.to_string());
    assert_eq!(receipt["result"]["revision"], 3);
    assert_eq!(receipt["result"]["published"], true);
    let item = fixture.get("/api/control/items/2").await;
    let events = fixture.get("/api/platform/events?after=0").await;
    fixture.restart().await;
    assert_eq!(
        fixture.post("/api/control/commands", &new_request).await,
        (StatusCode::OK, receipt)
    );
    assert_eq!(fixture.get("/api/control/items/2").await, item);
    assert_eq!(fixture.get("/api/platform/events?after=0").await, events);
    fixture.shutdown().await;
}
