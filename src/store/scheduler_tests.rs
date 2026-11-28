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
    for number in 1..=6 {
        f.store.ingest_event(ordinary_comment(number), policy).unwrap();
    }
    // A new ordinary input re-arms an expiring quiet window.
    f.db().execute("UPDATE wake_batches SET quiet_deadline='2000-01-01T00:00:00Z'", []).unwrap();
    let seventh = f.store.ingest_event(ordinary_comment(7), policy).unwrap();
    assert_eq!(seventh.batch_lifecycle.as_deref(), Some("pending"));
    f.store.advance_scheduler().unwrap();
    // Both transport redelivery and a new delivery of the same object version
    // must leave the threshold count at seven.
    assert!(f.store.ingest_event(ordinary_comment(7), policy).unwrap().duplicate);
    let mut redelivery = ordinary_comment(7);
    redelivery.delivery_guid = "new-delivery-same-comment".into();
    let repeated = f.store.ingest_event(redelivery, policy).unwrap();
    assert!(!repeated.duplicate);
    assert!(repeated.event_id.is_none());
    assert_eq!(repeated.event_lifecycle.as_deref(), Some("superseded"));
    assert!(
        f.store
            .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
            .unwrap()
            .is_none()
    );

    let eighth = f.store.ingest_event(ordinary_comment(8), policy).unwrap();
    assert_eq!(eighth.batch_lifecycle.as_deref(), Some("runnable"));
    let claim = f
        .store
        .claim_runnable_turn("issue".into(), "issue-codex".into(), vec!["provider".into()])
        .unwrap()
        .expect("the eighth distinct input is sufficient without a clock advance");
    assert_eq!(Some(claim.batch_id), eighth.batch_id);
    assert_eq!(claim.references, (1..=8).map(|n| format!("Comment {n}")).collect::<Vec<_>>());
}
