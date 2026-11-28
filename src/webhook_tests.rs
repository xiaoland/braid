use super::{ActorPolicy, WebhookHeaders, has_visible_mention, parse_verified};
use crate::store::{EventKind, IngressEvent};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde_json::{Value, json};
use sha2::Sha256;

fn payload() -> Value {
    json!({
        "action":"created", "repository":{"node_id":"R1","full_name":"owner/repository"},
        "sender":{"node_id":"HUMAN","login":"human"},
        "issue":{"node_id":"I1","number":1,"state":"open","body":"Current design"},
        "comment":{"id":7,"node_id":"C1","body":"@braid please review", "updated_at":"2026-01-01T00:00:00Z"}
    })
}

fn parse(payload: &Value, event: &str) -> IngressEvent {
    // Signing only prepares valid input for classification tests. The handler
    // tests independently verify fixed signed bytes and authentication failure.
    let body = serde_json::to_vec(payload).unwrap();
    let secret = b"classification-fixture";
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(&body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    parse_verified(
        WebhookHeaders {
            signature: Some(&signature),
            delivery: Some("classification"),
            event: Some(event),
        },
        &body,
        secret,
        &["owner/repository".into()],
        "braid",
        ActorPolicy {
            app_node_id: "APP",
            app_login: "braid[bot]",
            agent_node_ids: &["AGENT".into()],
        },
    )
    .unwrap()
}

#[test]
fn exact_visible_mention_excludes_code_quotes_comments_and_other_handles() {
    for (body, expected) in [
        ("@braid please review", true),
        ("Hello (@braid)!", true),
        ("a@braid", false),
        ("@braid-other", false),
        ("@braid_other", false),
        ("@braid2", false),
        ("@Braid", false),
        ("`@braid`", false),
        ("```text\n@braid\n```", false),
        ("> @braid", false),
        ("<!-- @braid -->", false),
        ("<!-- hidden -->\n\n@braid", true),
    ] {
        for handle in ["braid", "@braid"] {
            assert_eq!(has_visible_mention(body, handle), expected, "{body:?}, {handle}");
        }
    }
}

#[test]
fn opening_body_mention_remains_pending_and_agent_echo_cannot_request_attention() {
    let mut opening = payload();
    opening.as_object_mut().unwrap().remove("comment");
    opening["action"] = json!("opened");
    opening["issue"]["body"] = json!("@braid discuss this design");
    let admitted = parse(&opening, "issues");
    assert_eq!(admitted.kind, EventKind::Mention);
    assert!(admitted.mention_candidate);
    assert!(admitted.requires_attention);
    assert_eq!(admitted.origin, "external");
    for (node, login) in
        [("APP", "renamed-bot"), ("renamed-node", "braid[bot]"), ("AGENT", "coding-agent")]
    {
        let mut echo = payload();
        echo["sender"] = json!({"node_id":node,"login":login});
        let admitted = parse(&echo, "issue_comment");
        assert_eq!(admitted.kind, EventKind::OriginEcho);
        assert_eq!(admitted.origin, "agent");
        assert!(!admitted.mention_candidate);
        assert!(!admitted.requires_attention);
        assert!(admitted.reaction_target.is_none());
    }
    let human = parse(&payload(), "issue_comment");
    assert_eq!(human.kind, EventKind::Wake);
    assert!(human.mention_candidate);
    assert!(human.requires_attention);
    let reaction = human.reaction_target.unwrap();
    assert_eq!(reaction.kind, "issue_comment");
    assert_eq!(reaction.database_id, "7");
}

#[test]
fn comment_edits_compare_visible_suffix_instead_of_discarding_the_html_block_line() {
    for (body, expected) in [
        ("<!-- new hidden --> current design", EventKind::Noop),
        ("<!-- new hidden --> changed design", EventKind::Invalidate),
    ] {
        let mut edited = payload();
        edited["action"] = json!("edited");
        edited["changes"] = json!({"body":{"from":"<!-- old hidden --> current design"}});
        edited["comment"]["body"] = json!(body);
        let admitted = parse(&edited, "issue_comment");
        assert_eq!(admitted.kind, expected, "{body}");
        assert_eq!(
            admitted.visible_body.as_deref(),
            Some(if expected == EventKind::Noop { " current design" } else { " changed design" })
        );
        assert_eq!(admitted.requires_attention, expected == EventKind::Invalidate);
    }
}

#[test]
fn human_edit_of_a_braid_authored_body_is_not_an_echo_or_an_urgent_mention() {
    let mut revision = payload();
    revision.as_object_mut().unwrap().remove("comment");
    revision["action"] = json!("edited");
    revision["changes"] = json!({"body":{"from":"old requirements"}});
    revision["issue"]["body"] =
        json!("> **Braid Agent · Braid**\n> Issue Agent\n\nRevised requirements");
    let event = parse(&revision, "issues");
    assert_eq!(event.kind, EventKind::Invalidate);
    assert!(event.requires_attention);
    assert!(!event.mention_candidate);
    assert_eq!(event.origin, "external");
}
