//! A filtered view is not an observation of absence, and absence has no edit clock.
use super::*;
use crate::{
    context::{self, CanonicalContext, CommentSnapshot},
    producer::reconcile::{self, ReconcileScope},
    store::{ChangeIdentity, EventKind, ObservationBasis, ObservationCommit},
};

const PRESENT: &str = "2026-11-06T13:57:29Z";
const LOCAL_ABSENCE: &str = "2026-11-06T13:57:52.111442621Z";
fn comment(node: &str, body: &str) -> CommentSnapshot {
    CommentSnapshot {
        node_id: node.into(),
        database_id: "99".into(),
        repository: "owner/repository".into(),
        work_item_number: 1,
        author: Some(context::Actor { node_id: "app".into(), login: "braid[bot]".into() }),
        created_at: PRESENT.into(),
        updated_at: PRESENT.into(),
        last_edited_at: None,
        body: Some(body.into()),
        minimized: false,
        minimized_reason: None,
        pinned: false,
        deleted: false,
    }
}
fn scope(c: &CanonicalContext) -> ReconcileScope {
    let o = context::canonical_observations(c).remove(0);
    ReconcileScope {
        work_item_node_id: o.work_item_node_id,
        work_item_kind: o.work_item_kind,
        work_item_number: o.work_item_number,
        work_item_state: o.work_item_state.clone(),
        previous_work_item_state: o.work_item_state,
        repository_node_id: o.repository_node_id,
        repository: o.repository,
    }
}
fn observe(
    f: &Fixture,
    github: &GitHubClient,
    c: &CanonicalContext,
    basis: ObservationBasis,
) -> Result<usize> {
    reconcile::reconcile_observations(
        &f.store,
        github,
        &f.config,
        &scope(c),
        basis,
        &context::canonical_observations(c),
    )
}
async fn issue(
    f: &Fixture,
    node: &str,
) -> (CanonicalContext, Arc<GitHubClient>, tokio::task::JoinHandle<()>) {
    let (github, http) = issue_context_server("Current design").await;
    let locator = "owner/repository#1".parse().unwrap();
    let mut c =
        CanonicalContext::Issue(context::materialize_issue(&github, &locator, 100).await.unwrap());
    let CanonicalContext::Issue(i) = &mut c else { unreachable!() };
    i.comments = vec![comment(node, "current visible content")];
    context::reconcile_local_state(&mut c, &f.store, Some(&github.identity().actor_node_id))
        .unwrap();
    (c, github, http)
}
fn seed(f: &Fixture, c: &CanonicalContext) {
    let root = context::canonical_observations(c)[0].work_item_node_id.clone();
    f.store
        .commit_observation(ObservationCommit {
            basis: f.store.begin_observation(root).unwrap(),
            current: context::canonical_observations(c)
                .iter()
                .map(super::semantic_contract::processed)
                .collect(),
            events: vec![],
            policy: SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
        })
        .unwrap();
}
fn managed(f: &Fixture, node: &str, root: &str) {
    f.db().execute("INSERT INTO operational_comment_roles(role_id,work_item_node_id,profile_id,assignment_generation,comment_node_id) VALUES(?1,?2,'issue-codex',1,?3)",rusqlite::params!["a".repeat(32),root,node]).unwrap();
}
fn poison(f: &Fixture, node: &str) {
    for table in ["processed_objects", "canonical_objects"] {
        f.db()
            .execute(
                &format!("UPDATE {table} SET lifecycle='deleted',version=?2 WHERE node_id=?1"),
                rusqlite::params![node, LOCAL_ABSENCE],
            )
            .unwrap();
    }
}
fn raw(node: &str, guid: &str, deleted: bool) -> IngressEvent {
    let mut e = continuous_collaboration::revision(guid, "current visible content");
    e.identity = ChangeIdentity::Occurrence;
    e.event_name = "issue_comment".into();
    e.object_node_id = Some(node.into());
    e.object_version = Some(format!("{PRESENT}:{}:", if deleted { "deleted" } else { "active" }));
    e.action = Some(if deleted { "deleted" } else { "edited" }.into());
    e.detail = Some(if deleted { "deleted" } else { "edited" });
    e.cross_surface_invalidation = false;
    e.kind = EventKind::Invalidate;
    e
}

#[tokio::test]
async fn canonical_reads_recognize_a_status_create_whose_receipt_was_lost() {
    for producer_first in [true, false] {
        let f = Fixture::new(true);
        let remote = Arc::new(Mutex::new(body_edit::BodyRemote::new(false)));
        let (github, http) = body_edit::body_server(remote.clone()).await;
        f.store
            .enqueue_assignment_operational_status(
                "assignment".into(),
                "Provider unavailable".into(),
            )
            .unwrap();
        let owner =
            f.store.acquire_runtime_lease("runtime".into(), "publisher".into(), 3600).unwrap();
        let write = f.store.claim_github_write(owner.clone()).unwrap().unwrap();
        assert_eq!(write.operation, "comment_create");
        github.create_issue_comment(1, &write.content).await.unwrap();
        f.store.release_runtime_lease(owner).unwrap();
        f.store.recover_writes().unwrap();
        // The publisher stays off after the first remote mutation. Neither the
        // role claiming path nor the canonical observation can use its ACK.
        {
            let mut state = remote.lock().unwrap();
            let mut discussion = state.issue_comments[0].clone();
            discussion["id"] = json!("discussion");
            discussion["fullDatabaseId"] = json!("100");
            discussion["body"] = json!(write.content.replacen(
                "Provider unavailable",
                "Provider log reviewed in an ordinary App comment",
                1
            ));
            let mut imitation = state.issue_comments[0].clone();
            imitation["id"] = json!("imitation");
            imitation["fullDatabaseId"] = json!("101");
            imitation["author"] = json!({"id":"human", "login":"participant"});
            state.issue_comments.extend([discussion, imitation]);
        }
        let mut canonical = CanonicalContext::Issue(
            context::materialize_issue(&github, &"owner/repository#1".parse().unwrap(), 100)
                .await
                .unwrap(),
        );
        let basis = f.store.begin_observation("item".into()).unwrap();
        if producer_first {
            observe(&f, &github, &canonical, basis).unwrap();
            let status_observed: String = f
                .db()
                .query_row(
                    "SELECT kind FROM events WHERE object_node_id='status' AND detail='observed'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(status_observed, EventKind::Noop.as_str());
        }
        context::reconcile_local_state(
            &mut canonical,
            &f.store,
            Some(&github.identity().actor_node_id),
        )
        .unwrap();
        let CanonicalContext::Issue(issue) = &canonical else { unreachable!() };
        let visible =
            issue.comments.iter().map(|comment| comment.node_id.as_str()).collect::<Vec<_>>();
        assert_eq!(visible, ["discussion", "imitation"]);
        let rendered = context::render_complete(&canonical, 0.5, 100_000);
        assert!(rendered.text.contains("ordinary App comment"));
        assert!(!rendered.text.contains("braid-operational-status:v1:"));
        let db = f.db();
        let recognized: String = db
            .query_row("SELECT comment_node_id FROM operational_comment_roles", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(recognized, "status");
        let receipt: (Option<String>, String) = db
            .query_row(
                "SELECT s.remote_comment_node_id,o.lifecycle FROM status_comments s
             JOIN github_write_outbox o ON o.intent_id=s.write_intent_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(receipt, (None, "uncertain".into()));
        assert_eq!(
            db.query_row(
                "SELECT lifecycle FROM canonical_objects WHERE node_id='status'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "active"
        );
        http.abort();
    }
}

#[tokio::test]
async fn operational_projection_never_turns_observed_members_into_tombstones() {
    for pr in [false, true] {
        let f = Fixture::new(true);
        let (github, http) = issue_context_server("Current design").await;
        let locator = if pr { "owner/repository#9" } else { "owner/repository#1" }.parse().unwrap();
        let mut c = if pr {
            CanonicalContext::PullRequest(
                context::materialize_pull_request(&github, &locator, 100).await.unwrap(),
            )
        } else {
            CanonicalContext::Issue(
                context::materialize_issue(&github, &locator, 100).await.unwrap(),
            )
        };
        context::reconcile_local_state(&mut c, &f.store, Some(&github.identity().actor_node_id))
            .unwrap();
        let root = scope(&c).work_item_node_id;
        managed(&f, "status", &root);
        let mut echo = raw("status", "status-created", false);
        echo.work_item_node_id = Some(root.clone());
        echo.work_item_kind = Some(if pr { "pr" } else { "issue" });
        echo.work_item_number = Some(if pr { 9 } else { 1 });
        echo.kind = EventKind::OriginEcho;
        echo.origin = "agent";
        f.store
            .ingest_event(echo, SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 })
            .unwrap();
        let comments = match &mut c {
            CanonicalContext::Issue(i) => &mut i.comments,
            CanonicalContext::PullRequest(p) => &mut p.conversation,
        };
        comments.extend([
            comment("status", "private operational diagnostic"),
            comment("ordinary", "> **Braid Operational Status** is just user prose"),
        ]);
        context::reconcile_local_state(&mut c, &f.store, Some(&github.identity().actor_node_id))
            .unwrap();
        let lifecycle: String = f
            .db()
            .query_row("SELECT lifecycle FROM canonical_objects WHERE node_id='status'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(lifecycle, "active");
        let rendered = context::render_complete(&c, 0.5, 100_000);
        assert!(!rendered.text.contains("private operational diagnostic"));
        assert!(rendered.text.contains("is just user prose"));
        http.abort();
    }
}

#[tokio::test]
async fn a_legacy_local_tombstone_is_corrected_by_presence_without_waking_for_status() {
    for owned in [false, true] {
        let f = Fixture::new(true);
        let (c, github, http) = issue(&f, "comment").await;
        if owned {
            managed(&f, "comment", "item");
        }
        seed(&f, &c);
        poison(&f, "comment");
        let denied = f
            .store
            .ingest_event(
                raw("comment", "old-visible-event", false),
                SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
            )
            .unwrap();
        assert_eq!(
            denied.event_lifecycle.as_deref(),
            Some("superseded"),
            "a late webhook cannot restore a tombstone"
        );
        let basis = f.store.begin_observation("item".into()).unwrap();
        assert_eq!(observe(&f, &github, &c, basis).unwrap(), 1);
        let restored = f
            .store
            .begin_observation("item".into())
            .unwrap()
            .objects
            .into_iter()
            .find(|o| o.node_id == "comment")
            .unwrap();
        assert_eq!(restored.lifecycle, "active");
        assert!(restored.version.starts_with(PRESENT));
        assert_eq!(f.store.runtime_status().unwrap().batches.len(), usize::from(!owned));
        assert_eq!(
            observe(&f, &github, &c, f.store.begin_observation("item".into()).unwrap()).unwrap(),
            0
        );
        http.abort();
    }
}

#[tokio::test]
async fn an_admitted_delete_during_the_api_read_rejects_the_whole_old_snapshot() {
    let f = Fixture::new(true);
    let (c, github, http) = issue(&f, "comment").await;
    seed(&f, &c);
    let basis = f.store.begin_observation("item".into()).unwrap();
    f.store
        .ingest_event(
            raw("comment", "delete-during-read", true),
            SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
        )
        .unwrap();
    let expected = f.store.begin_observation("item".into()).unwrap().objects;
    let error = observe(&f, &github, &c, basis).unwrap_err().to_string();
    assert!(error.contains("events arrived during canonical traversal"), "{error}");
    assert_eq!(f.store.begin_observation("item".into()).unwrap().objects, expected);
    http.abort();
}

#[tokio::test]
async fn truly_newer_positive_edits_still_fence_older_snapshots_and_name_the_conflict() {
    let f = Fixture::new(true);
    let (mut c, github, http) = issue(&f, "comment").await;
    let CanonicalContext::Issue(i) = &mut c else { unreachable!() };
    i.comments[0].updated_at = "2026-11-06T14:00:00Z".into();
    seed(&f, &c);
    let CanonicalContext::Issue(i) = &mut c else { unreachable!() };
    i.comments[0].updated_at = PRESENT.into();
    let error = observe(&f, &github, &c, f.store.begin_observation("item".into()).unwrap())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("comment")
            && error.contains("14:00:00Z")
            && error.contains(PRESENT)
            && error.contains("active"),
        "{error}"
    );
    http.abort();
}
