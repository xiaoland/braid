//! Product oracles for repeated state transitions and non-semantic edits.
use super::*;
use crate::{
    context::{self, CanonicalContext, CommentSnapshot, ReviewThreadSnapshot},
    store::{CanonicalObjectState, ChangeIdentity, EventKind, ObservationCommit},
};

fn policy() -> SchedulerPolicy {
    SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 }
}
fn comment(body: &str) -> CommentSnapshot {
    CommentSnapshot {
        node_id: "comment".into(),
        database_id: "99".into(),
        repository: "owner/repository".into(),
        work_item_number: 1,
        author: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
        last_edited_at: None,
        body: Some(body.into()),
        minimized: false,
        minimized_reason: None,
        pinned: false,
        deleted: false,
    }
}
fn processed(o: &context::CanonicalObservation) -> CanonicalObjectState {
    CanonicalObjectState {
        node_id: o.object_node_id.clone(),
        database_id: Some(o.database_id.clone()),
        object_kind: o.object_kind.into(),
        version: o.version.clone(),
        digest: o.digest.clone(),
        lifecycle: if matches!(o.object_kind, "issue" | "pr") {
            o.work_item_state.to_ascii_lowercase()
        } else {
            o.lifecycle.into()
        },
        author_node_id: o.author_node_id.clone(),
        author_login: o.author_login.clone(),
        metadata_digest: o.metadata_digest.clone(),
        root_fields: o.root_fields(),
    }
}
pub(super) fn observe(f: &Fixture, github: &GitHubClient, current: &CanonicalContext) -> usize {
    let observations = context::canonical_observations(current);
    let root = &observations[0];
    let basis = f.store.begin_observation(root.work_item_node_id.clone()).unwrap();
    let previous = basis
        .objects
        .iter()
        .find(|o| o.node_id == root.work_item_node_id)
        .map_or(root.work_item_state.clone(), |o| o.lifecycle.clone());
    let scope = crate::producer::reconcile::ReconcileScope {
        work_item_node_id: root.work_item_node_id.clone(),
        work_item_kind: root.work_item_kind,
        work_item_number: root.work_item_number,
        work_item_state: root.work_item_state.clone(),
        previous_work_item_state: previous,
        repository_node_id: root.repository_node_id.clone(),
        repository: root.repository.clone(),
    };
    crate::producer::reconcile::reconcile_observations(
        &f.store,
        github,
        &f.config,
        &scope,
        basis,
        &observations,
    )
    .unwrap()
}
fn thread_state(current: &mut CanonicalContext, resolved: bool) {
    let CanonicalContext::PullRequest(pr) = current else { unreachable!() };
    pr.review_threads[0].resolved = resolved;
    pr.review_threads[0].resolved_by =
        resolved.then(|| context::Actor { node_id: "human".into(), login: "human".into() });
}
fn restorations(f: &Fixture) -> i64 {
    f.db()
        .query_row(
            "SELECT count(*) FROM events WHERE detail='unresolved' AND kind='wake'",
            [],
            |r| r.get(0),
        )
        .unwrap()
}
fn raw_restoration(id: &str, target: &str, number: u64) -> IngressEvent {
    let mut event = continuous_collaboration::revision(id, "");
    event.identity = ChangeIdentity::Occurrence;
    event.event_name = "pull_request_review_thread".into();
    event.action = Some("unresolved".into());
    event.detail = Some("unresolved");
    event.work_item_node_id = Some(target.into());
    event.work_item_kind = Some("pr");
    event.work_item_number = Some(number);
    event.object_node_id = Some("thread".into());
    event.object_version = None;
    event.object_digest = None;
    event.visible_body = None;
    event.cross_surface_invalidation = false;
    event.kind = EventKind::Wake;
    event
}

#[test]
fn contract_versionless_restoration_is_a_new_occurrence_not_a_redelivery() {
    let f = Fixture::new(false);
    for (id, resolved) in
        [("first-unresolve", false), ("then-resolve", true), ("second-unresolve", false)]
    {
        let mut event = raw_restoration(id, "item", 1);
        if resolved {
            event.kind = EventKind::Invalidate;
            event.action = Some("resolved".into());
            event.detail = Some("resolved");
        }
        let admitted = f.store.ingest_event(event.clone(), policy()).unwrap();
        assert!(admitted.event_id.is_some(), "distinct restoration was collapsed: {id}");
        assert!(f.store.ingest_event(event, policy()).unwrap().duplicate);
    }
    assert_eq!(restorations(&f), 2);
    assert!(
        f.store.runtime_status().unwrap().agent_groups.is_empty(),
        "restoration must not activate a dormant responsibility"
    );
}

#[tokio::test]
async fn contract_hidden_comment_edit_is_not_new_work_during_reconciliation() {
    let f = Fixture::new(true);
    let (github, http) = issue_context_server("Accepted design").await;
    let locator = "owner/repository#1".parse().unwrap();
    let mut current =
        CanonicalContext::Issue(context::materialize_issue(&github, &locator, 100).await.unwrap());
    let CanonicalContext::Issue(issue) = &mut current else { unreachable!() };
    issue.comments = vec![comment("Visible agreement<!-- old hidden note -->")];
    let prior = context::canonical_observations(&current).iter().map(processed).collect();
    f.store
        .commit_observation(ObservationCommit {
            basis: f.store.begin_observation("item".into()).unwrap(),
            current: prior,
            events: vec![],
            policy: policy(),
        })
        .unwrap();
    let CanonicalContext::Issue(issue) = &mut current else { unreachable!() };
    issue.comments[0].updated_at = "2026-01-01T00:00:01Z".into();
    issue.comments[0].last_edited_at = Some("2026-01-01T00:00:01Z".into());
    issue.comments[0].body = Some("Visible agreement<!-- different hidden note -->".into());
    assert_eq!(
        observe(&f, &github, &current),
        0,
        "a hidden/timestamp-only edit cannot create work"
    );
    assert!(f.store.runtime_status().unwrap().batches.is_empty());
    let CanonicalContext::Issue(issue) = &mut current else { unreachable!() };
    issue.comments[0].body = Some("Visible revised agreement".into());
    issue.comments[0].updated_at = "2026-01-01T00:00:02Z".into();
    assert_eq!(observe(&f, &github, &current), 1);
    assert_eq!(observe(&f, &github, &current), 0);
    http.abort();
}

#[tokio::test]
async fn contract_restoration_cycles_survive_polling_and_share_a_real_ingress_receipt() {
    let f = Fixture::new(false);
    let (github, http) = issue_context_server("Accepted design").await;
    let locator = "owner/repository#9".parse().unwrap();
    let mut current = CanonicalContext::PullRequest(
        context::materialize_pull_request(&github, &locator, 100).await.unwrap(),
    );
    let CanonicalContext::PullRequest(pr) = &mut current else { unreachable!() };
    let mut discussion = comment("Review discussion");
    discussion.work_item_number = 9;
    pr.review_threads = vec![ReviewThreadSnapshot {
        node_id: "thread".into(),
        path: "file.rs".into(),
        line: Some(10),
        start_line: None,
        resolved: true,
        resolved_by: None,
        collapsed: false,
        outdated: false,
        comments: vec![discussion],
    }];
    observe(&f, &github, &current);
    for resolved in [false, true, false] {
        thread_state(&mut current, resolved);
        assert_eq!(observe(&f, &github, &current), 1);
        assert_eq!(observe(&f, &github, &current), 0);
    }
    assert_eq!(restorations(&f), 2, "a repeated state is a new transition after resolution");
    thread_state(&mut current, true);
    observe(&f, &github, &current);
    let event = raw_restoration("webhook-restoration", "pr", 9);
    let accepted = f.store.ingest_event(event.clone(), policy()).unwrap().event_id.unwrap();
    thread_state(&mut current, false);
    assert_eq!(
        observe(&f, &github, &current),
        0,
        "canonical polling must reuse the already admitted restoration"
    );
    assert_eq!(restorations(&f), 3);
    let linked:String=f.db().query_row("SELECT effect_event_id FROM deliveries WHERE event_name='pull_request_review_thread' AND action='unresolved' ORDER BY rowid DESC LIMIT 1",[],|r|r.get(0)).unwrap();
    assert_eq!(linked, accepted);
    assert!(f.store.ingest_event(event, policy()).unwrap().duplicate);
    // Reusing an admitted restoration cannot swallow an independent position edit.
    thread_state(&mut current, true);
    observe(&f, &github, &current);
    f.store.ingest_event(raw_restoration("another-restoration", "pr", 9), policy()).unwrap();
    let basis = f.store.begin_observation("pr".into()).unwrap();
    let expected = basis.objects.clone();
    thread_state(&mut current, false);
    let CanonicalContext::PullRequest(pr) = &mut current else { unreachable!() };
    pr.review_threads[0].path = "different-file.rs".into();
    let mut invalid = raw_restoration("reconcile-invalid-reuse", "pr", 9);
    invalid.identity = ChangeIdentity::ObservedTransition;
    assert!(
        f.store
            .commit_observation(ObservationCommit {
                basis,
                current: context::canonical_observations(&current).iter().map(processed).collect(),
                events: vec![invalid],
                policy: policy()
            })
            .unwrap_err()
            .to_string()
            .contains(
                "state receipt cannot acknowledge changed metadata or an unproven transition"
            )
    );
    assert_eq!(f.store.begin_observation("pr".into()).unwrap().objects, expected);
    http.abort();
}
