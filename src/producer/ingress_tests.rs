use super::{IngressState, SchedulerPolicy, State, StatusCode, StoreActor, webhook_handler};
use axum::{
    body::{Bytes, to_bytes},
    http::HeaderMap,
};
use rusqlite::Connection;
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};

// Exact wire bytes and HMACs computed independently with Python's hmac/SHA256.
// The test never asks the verifier to manufacture its own expected signature.
const SECRET: &[u8] = b"ingress-fixture-secret";
const PRIMARY: &str = r#"{"action":"created","repository":{"node_id":"R1","full_name":"owner/repository"},"sender":{"login":"human","node_id":"H1"},"issue":{"node_id":"I1","number":1,"state":"open"},"comment":{"id":7,"node_id":"C1","body":"Current design","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}}"#;
const PRIMARY_SIGNATURE: &str =
    "sha256=d2889d3d4a96b8dd0cea939dbb527c8ce517e187a1e26656e140896b7816e86b";
const SECONDARY: &str = r#"{"action":"created","repository":{"node_id":"R2","full_name":"owner/other"},"sender":{"login":"human","node_id":"H1"},"issue":{"node_id":"I2","number":1,"state":"open"},"comment":{"id":7,"node_id":"C2","body":"Current design","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}}"#;
const SECONDARY_SIGNATURE: &str =
    "sha256=6e656aaae9ad1984276b2b8d1d47c35ede48afdafb7cca1df2ec5172bc304449";

struct Database(PathBuf);
impl Database {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("braid-webhook-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
    fn state(&self, repositories: &[&str]) -> Arc<IngressState> {
        let store =
            StoreActor::start(self.0.join("state.sqlite3"), self.0.join("backups")).unwrap();
        store.apply().unwrap();
        Arc::new(IngressState {
            store: Arc::new(store),
            policy: SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
            repositories: repositories.iter().map(|name| (*name).into()).collect(),
            handle: "braid".into(),
            app_actor_node_id: "APP".into(),
            app_actor_login: "braid[bot]".into(),
            agent_actor_node_ids: vec![],
            webhook_secret: Arc::new(SECRET.to_vec()),
        })
    }
    fn connect(&self) -> Connection {
        Connection::open(self.0.join("state.sqlite3")).unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        self.connect()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
            .unwrap()
    }
}
impl Drop for Database {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

async fn deliver(
    state: &Arc<IngressState>,
    body: &str,
    signature: Option<&str>,
    guid: &str,
) -> (StatusCode, String) {
    let mut headers = HeaderMap::new();
    headers.insert("x-github-delivery", guid.parse().unwrap());
    headers.insert("x-github-event", "issue_comment".parse().unwrap());
    if let Some(signature) = signature {
        headers.insert("x-hub-signature-256", signature.parse().unwrap());
    }
    let response =
        webhook_handler(State(state.clone()), headers, Bytes::copy_from_slice(body.as_bytes()))
            .await;
    let status = response.status();
    let body = to_bytes(response.into_body(), 65_536).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}

#[tokio::test]
async fn webhook_rejects_unverified_bytes_and_unconfigured_repository_before_durable_ingest() {
    let db = Database::new();
    let state = db.state(&["owner/repository"]);
    let changed_bytes = format!("{PRIMARY} "); // Same JSON value, different signed bytes.
    for (body, signature, reason) in [
        (PRIMARY, None, "missing X-Hub-Signature-256"),
        (PRIMARY, Some("sha256=not-hex"), "invalid X-Hub-Signature-256"),
        (changed_bytes.as_str(), Some(PRIMARY_SIGNATURE), "invalid X-Hub-Signature-256"),
    ] {
        let (status, error) = deliver(&state, body, signature, "rejected").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(error, reason);
    }
    let (status, error) =
        deliver(&state, SECONDARY, Some(SECONDARY_SIGNATURE), "wrong-repository").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error,
        "webhook repository \"owner/other\" does not match configured repository \"owner/repository\""
    );
    assert_eq!(db.count("deliveries"), 0);
    assert_eq!(db.count("events"), 0);
    assert_eq!(db.count("work_items"), 0);
    let (status, body) = deliver(&state, PRIMARY, Some(PRIMARY_SIGNATURE), "accepted").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["duplicate"], false);
    assert_eq!(db.count("deliveries"), 1);
}

#[tokio::test]
async fn signed_same_number_repositories_keep_distinct_events_and_deduplicate_after_actor_restart()
{
    let db = Database::new();
    let repositories = ["owner/repository", "owner/other"];
    let state = db.state(&repositories);
    for (body, signature, guid) in [
        (PRIMARY, PRIMARY_SIGNATURE, "delivery-primary"),
        (SECONDARY, SECONDARY_SIGNATURE, "delivery-secondary"),
    ] {
        let (status, body) = deliver(&state, body, Some(signature), guid).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{body}");
        let receipt: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(receipt["duplicate"], false);
        assert!(receipt["event_id"].as_str().is_some());
    }
    drop(state); // No actor or in-memory deduplication survives.
    let state = db.state(&repositories);
    for (body, signature, guid) in [
        (PRIMARY, PRIMARY_SIGNATURE, "delivery-primary"),
        (SECONDARY, SECONDARY_SIGNATURE, "delivery-secondary"),
    ] {
        let (status, body) = deliver(&state, body, Some(signature), guid).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["duplicate"], true);
    }
    assert_eq!(db.count("deliveries"), 2);
    assert_eq!(db.count("events"), 2);
    assert_eq!(db.count("work_items"), 2);
    let connection = db.connect();
    let mut statement = connection.prepare("SELECT d.repository_name,w.node_id,w.number,e.reference,d.raw_payload FROM deliveries d JOIN events e ON e.delivery_guid=d.delivery_guid JOIN work_items w ON w.node_id=e.work_item_node_id").unwrap();
    let actual: BTreeMap<String, (String, i64, String, Vec<u8>)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, (row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        actual,
        BTreeMap::from([
            (
                "owner/repository".into(),
                (
                    "I1".into(),
                    1,
                    "GitHub comment owner/repository#issuecomment-7 was created by @human.".into(),
                    PRIMARY.as_bytes().to_vec()
                )
            ),
            (
                "owner/other".into(),
                (
                    "I2".into(),
                    1,
                    "GitHub comment owner/other#issuecomment-7 was created by @human.".into(),
                    SECONDARY.as_bytes().to_vec()
                )
            ),
        ])
    );
}

#[tokio::test]
async fn signed_app_comment_mention_does_not_schedule_a_handoff_or_wake() {
    const BODY: &str = r#"{"action":"created","repository":{"node_id":"R1","full_name":"owner/repository"},"sender":{"login":"braid[bot]","node_id":"APP"},"issue":{"node_id":"I1","number":1,"state":"open","pull_request":{}},"comment":{"id":7,"node_id":"C1","body":"@braid continue this handoff","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}}"#;
    let db = Database::new();
    let state = db.state(&["owner/repository"]);
    db.connect()
        .execute_batch(
            "INSERT INTO repositories VALUES ('R1','owner/repository',1,'now');
         INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
             VALUES ('I1','R1','pr',1,'open','now');
         INSERT INTO assignments VALUES ('current','I1',1,'active','now',NULL);",
        )
        .unwrap();
    let (status, receipt) = deliver(
        &state,
        BODY,
        Some("sha256=cc52441234b35bf3359ce89a5d58821e6d65ab784f8e87d7880fe4478da628f3"),
        "app-echo",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let id: Value = serde_json::from_str(&receipt).unwrap();
    let kind: String = db
        .connect()
        .query_row(
            "SELECT kind FROM events WHERE event_id=?1",
            [id["event_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(kind, "origin_echo");
    assert_eq!(db.count("wake_batches"), 0);
    assert_eq!(db.count("pr_handoffs"), 0);
}
