use super::{ChangeIdentity, EventKind, IngressEvent, SchedulerPolicy};
use crate::writer::tests::Fixture;

fn ordinary_comment(number: u32) -> IngressEvent {
    IngressEvent {
        identity: ChangeIdentity::ObjectVersion,
        delivery_guid: format!("delivery-{number}"),
        event_name: "issue_comment".into(),
        action: Some("created".into()),
        repository_node_id: "repo".into(),
        repository: "owner/repository".into(),
        work_item_node_id: Some("item".into()),
        work_item_kind: Some("issue"),
        work_item_number: Some(1),
        work_item_state: Some("open".into()),
        object_node_id: Some(format!("comment-{number}")),
        object_version: Some("2026-01-01T00:00:00Z".into()),
        object_digest: Some("a".repeat(64)),
        visible_body: Some(format!("Comment {number}")),
        actor_node_id: Some("human".into()),
        actor_login: Some("human".into()),
        kind: EventKind::Wake,
        detail: Some("created"),
        cross_surface_invalidation: false,
        requires_attention: true,
        origin: "external",
        reference: format!("Comment {number}"),
        mention_candidate: false,
        reaction_target: None,
        known: true,
        raw_payload: vec![],
    }
}

#[test]
fn eighth_distinct_input_releases_the_batch_without_waiting_for_quiet() {
    let f = Fixture::new(true);
    let policy = SchedulerPolicy { quiet_seconds: 3600, event_threshold: 8 };
    for number in 1..=7 {
        let admitted = f.store.ingest_event(ordinary_comment(number), policy).unwrap();
        assert!(admitted.event_id.is_some());
        assert_eq!(admitted.batch_lifecycle.as_deref(), Some("pending"));
    }
    assert_eq!(f.store.advance_scheduler().unwrap(), 0);
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );
    // Both transport redelivery and a new delivery of the same object version
    // must leave the threshold count at seven.
    assert!(f.store.ingest_event(ordinary_comment(7), policy).unwrap().duplicate);
    let mut redelivery = ordinary_comment(7);
    redelivery.delivery_guid = "new-delivery-same-comment".into();
    let repeated = f.store.ingest_event(redelivery, policy).unwrap();
    assert!(!repeated.duplicate);
    assert!(repeated.event_id.is_none());
    assert_eq!(repeated.event_lifecycle.as_deref(), Some("superseded"));
    let status = f.store.runtime_status().unwrap();
    assert_eq!((status.pending_batches, status.runnable_batches), (1, 0));
    assert_eq!(status.batches.len(), 1);
    assert_eq!(status.batches[0].event_count, 7);
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );

    let eighth = f.store.ingest_event(ordinary_comment(8), policy).unwrap();
    assert_eq!(eighth.batch_lifecycle.as_deref(), Some("runnable"));
    assert_eq!(f.store.runtime_status().unwrap().batches[0].event_count, 8);
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
        .unwrap()
        .expect("the eighth distinct input is sufficient without a clock advance");
    assert_eq!(Some(claim.batch_id), eighth.batch_id);
    assert_eq!(claim.references, (1..=8).map(|n| format!("Comment {n}")).collect::<Vec<_>>());
    assert_eq!(claim.trigger_kind, "wake_batch");
    assert!(!claim.trusted_mention);
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );
    assert!(f.store.runtime_status().unwrap().batches.is_empty());
}

#[test]
fn quiet_deadline_releases_a_subthreshold_batch_only_when_due() {
    let f = Fixture::new(true);
    let policy = SchedulerPolicy { quiet_seconds: 3600, event_threshold: 8 };
    let admitted = f.store.ingest_event(ordinary_comment(1), policy).unwrap();
    assert_eq!(admitted.batch_lifecycle.as_deref(), Some("pending"));
    assert_eq!(f.store.advance_scheduler().unwrap(), 0);
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );
    // Move only the persisted deadline across the clock boundary; real ingestion,
    // scheduler advancement and claiming still own every lifecycle transition.
    assert_eq!(
        f.db()
            .execute(
                "UPDATE wake_batches SET quiet_deadline='2000-01-01T00:00:00Z' WHERE batch_id=?1",
                [admitted.batch_id.as_deref().unwrap()],
            )
            .unwrap(),
        1
    );
    assert_eq!(f.store.advance_scheduler().unwrap(), 1);
    assert_eq!(f.store.advance_scheduler().unwrap(), 0);
    let status = f.store.runtime_status().unwrap();
    assert_eq!((status.pending_batches, status.runnable_batches), (0, 1));
    assert_eq!(status.batches[0].event_count, 1);
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
        .unwrap()
        .expect("an expired quiet window is sufficient below the count threshold");
    assert_eq!(Some(claim.batch_id), admitted.batch_id);
    assert_eq!(claim.references, vec!["Comment 1"]);
    assert_eq!(claim.trigger_kind, "wake_batch");
    assert!(!claim.trusted_mention);
}
