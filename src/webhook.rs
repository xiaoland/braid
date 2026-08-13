use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use hmac::{Hmac, KeyInit as _, Mac as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::store::{IngressEvent, ReactionTarget};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum WebhookError {
    #[error("missing X-Hub-Signature-256")]
    MissingSignature,
    #[error("invalid X-Hub-Signature-256")]
    InvalidSignature,
    #[error("missing X-GitHub-Delivery")]
    MissingDelivery,
    #[error("missing X-GitHub-Event")]
    MissingEvent,
    #[error("unsupported webhook JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("webhook repository {actual:?} does not match configured repository {expected:?}")]
    WrongRepository { actual: String, expected: String },
    #[error("webhook omitted repository identity")]
    MissingRepository,
}

#[derive(Debug, Clone, Copy)]
pub struct WebhookHeaders<'a> {
    pub signature: Option<&'a str>,
    pub delivery: Option<&'a str>,
    pub event: Option<&'a str>,
}

pub fn parse_verified(
    headers: WebhookHeaders<'_>,
    body: &[u8],
    secret: &[u8],
    configured_repository: &str,
    configured_handle: &str,
    app_actor_node_id: &str,
    app_actor_login: &str,
) -> Result<IngressEvent, WebhookError> {
    verify_signature(headers.signature.ok_or(WebhookError::MissingSignature)?, body, secret)?;
    let delivery = headers.delivery.ok_or(WebhookError::MissingDelivery)?;
    let event_name = headers.event.ok_or(WebhookError::MissingEvent)?;
    let payload: Payload = serde_json::from_slice(body)?;
    let repository = payload.repository.as_ref().ok_or(WebhookError::MissingRepository)?;
    if repository.full_name != configured_repository {
        return Err(WebhookError::WrongRepository {
            actual: repository.full_name.clone(),
            expected: configured_repository.into(),
        });
    }

    let action = payload.action.clone();
    let actor_node_id = payload.sender.as_ref().and_then(|actor| actor.node_id.clone());
    let actor_login = payload.sender.as_ref().map(|actor| actor.login.clone());
    let agent_origin = actor_node_id.as_deref() == Some(app_actor_node_id)
        || actor_login.as_deref() == Some(app_actor_login);
    let target = target(event_name, &payload, action.as_deref());
    let known = is_known_event(event_name);
    let classification =
        if agent_origin { "agent_origin" } else { classify(event_name, action.as_deref()) };
    let body_text = payload
        .comment
        .as_ref()
        .and_then(|comment| comment.body.as_deref())
        .or_else(|| payload.review.as_ref().and_then(|review| review.body.as_deref()));
    let mention_candidate = !agent_origin
        && matches!(action.as_deref(), Some("created" | "edited"))
        && body_text.is_some_and(|body| has_visible_mention(body, configured_handle));
    let reaction_target = if !agent_origin && action.as_deref() == Some("created") {
        match event_name {
            "issue_comment" => payload.comment.as_ref().map(|comment| ReactionTarget {
                kind: "issue_comment",
                database_id: comment.id.to_string(),
            }),
            "pull_request_review_comment" => payload.comment.as_ref().map(|comment| {
                ReactionTarget { kind: "review_comment", database_id: comment.id.to_string() }
            }),
            _ => None,
        }
    } else {
        None
    };
    let object_digest = match (
        target.object_node_id.as_deref(),
        target.object_version.as_deref(),
        target.object_body.as_deref(),
    ) {
        (Some(node_id), Some(version), _) if matches!(event_name, "issues" | "pull_request") => {
            target.work_item_state.as_deref().map(|state| root_digest(node_id, state, version))
        }
        (Some(node_id), Some(version), body) => Some(object_digest(node_id, version, body)),
        _ => None,
    };
    Ok(IngressEvent {
        delivery_guid: delivery.into(),
        event_name: event_name.into(),
        action,
        repository_node_id: repository.node_id.clone(),
        repository: repository.full_name.clone(),
        work_item_node_id: target.work_item_node_id,
        work_item_kind: target.work_item_kind,
        work_item_number: target.work_item_number,
        work_item_state: target.work_item_state,
        object_node_id: target.object_node_id,
        object_version: target.object_version,
        object_digest,
        actor_node_id,
        actor_login,
        classification,
        origin: if agent_origin { "agent" } else { "external" },
        reference: target.reference,
        mention_candidate,
        reaction_target,
        known,
        raw_payload: body.to_vec(),
    })
}

fn verify_signature(signature: &str, body: &[u8], secret: &[u8]) -> Result<(), WebhookError> {
    let encoded = signature.strip_prefix("sha256=").ok_or(WebhookError::InvalidSignature)?;
    let expected = hex::decode(encoded).map_err(|_| WebhookError::InvalidSignature)?;
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| WebhookError::InvalidSignature)?;
    mac.update(body);
    mac.verify_slice(&expected).map_err(|_| WebhookError::InvalidSignature)
}

fn is_known_event(event_name: &str) -> bool {
    matches!(
        event_name,
        "ping"
            | "issues"
            | "issue_comment"
            | "issue_dependencies"
            | "pull_request"
            | "pull_request_review"
            | "pull_request_review_comment"
            | "pull_request_review_thread"
            | "project_v2_item"
            | "installation"
            | "installation_repositories"
            | "repository"
    )
}

fn classify(event_name: &str, action: Option<&str>) -> &'static str {
    match (event_name, action) {
        ("issue_comment" | "pull_request_review_comment", Some("created"))
        | ("pull_request_review", Some("submitted"))
        | ("pull_request_review_thread", Some("unresolved"))
        | ("pull_request", Some("synchronize" | "review_requested" | "reopened")) => "wake",
        ("issues", Some("assigned" | "unassigned" | "closed" | "reopened"))
        | ("pull_request", Some("closed")) => "lifecycle",
        (
            "issues"
            | "issue_comment"
            | "pull_request"
            | "pull_request_review"
            | "pull_request_review_comment",
            Some("edited" | "deleted"),
        )
        | ("pull_request_review", Some("dismissed"))
        | ("pull_request_review_thread", Some("resolved")) => "hard_invalidation",
        ("ping", _) => "no_wake",
        _ => "unknown",
    }
}

#[derive(Default)]
struct Target {
    work_item_node_id: Option<String>,
    work_item_kind: Option<&'static str>,
    work_item_number: Option<u64>,
    work_item_state: Option<String>,
    object_node_id: Option<String>,
    object_version: Option<String>,
    object_body: Option<String>,
    reference: String,
}

fn target(event_name: &str, payload: &Payload, action: Option<&str>) -> Target {
    let work_item = if matches!(event_name, "issues" | "issue_comment" | "issue_dependencies") {
        payload.issue.as_ref().map(|issue| {
            let kind = if issue.pull_request.is_some() { "pr" } else { "issue" };
            (issue.node_id.clone(), kind, issue.number, issue.state.clone())
        })
    } else {
        payload.pull_request.as_ref().map(|pull_request| {
            (pull_request.node_id.clone(), "pr", pull_request.number, pull_request.state.clone())
        })
    };
    let object = payload
        .comment
        .as_ref()
        .map(|comment| {
            let timestamp = comment.updated_at.clone().or_else(|| comment.created_at.clone());
            let version = timestamp.map(|timestamp| {
                let lifecycle = if action == Some("deleted") { "deleted" } else { "active" };
                format!("{timestamp}:{lifecycle}:")
            });
            (comment.node_id.clone(), version, comment.body.clone())
        })
        .or_else(|| {
            payload.review.as_ref().map(|review| {
                (
                    review.node_id.clone(),
                    review.updated_at.clone().or_else(|| review.submitted_at.clone()),
                    review.body.clone(),
                )
            })
        })
        .or_else(|| {
            payload
                .thread
                .as_ref()
                .map(|thread| (thread.node_id.clone(), thread.updated_at.clone(), None))
        })
        .or_else(|| {
            payload.pull_request.as_ref().map(|pull_request| {
                (
                    pull_request.node_id.clone(),
                    pull_request.updated_at.clone(),
                    pull_request.body.clone(),
                )
            })
        })
        .or_else(|| {
            payload
                .issue
                .as_ref()
                .map(|issue| (issue.node_id.clone(), issue.updated_at.clone(), issue.body.clone()))
        });
    let reference = match (&work_item, &object) {
        (Some((_, kind, number, _)), Some((node_id, version, _))) => format!(
            "GitHub {kind} {}#{} object {} at {}",
            payload
                .repository
                .as_ref()
                .map_or("unknown", |repository| repository.full_name.as_str()),
            number,
            node_id,
            version.as_deref().unwrap_or("unknown version")
        ),
        (Some((_, kind, number, _)), None) => format!(
            "GitHub {kind} {}#{}",
            payload
                .repository
                .as_ref()
                .map_or("unknown", |repository| repository.full_name.as_str()),
            number
        ),
        _ => format!("GitHub webhook {event_name}"),
    };
    Target {
        work_item_node_id: work_item.as_ref().map(|value| value.0.clone()),
        work_item_kind: work_item.as_ref().map(|value| value.1),
        work_item_number: work_item.as_ref().map(|value| value.2),
        work_item_state: work_item.map(|value| value.3),
        object_node_id: object.as_ref().map(|value| value.0.clone()),
        object_version: object.as_ref().and_then(|value| value.1.clone()),
        object_body: object.and_then(|value| value.2),
        reference,
    }
}

fn object_digest(node_id: &str, version: &str, body: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(node_id.as_bytes());
    digest.update(b"\0");
    digest.update(version.as_bytes());
    digest.update(b"\0");
    if let Some(body) = body {
        digest.update(body.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn root_digest(node_id: &str, state: &str, version: &str) -> String {
    let mut digest = Sha256::new();
    for value in [node_id, state, version] {
        digest.update(value.as_bytes());
        digest.update(b"\0");
    }
    hex::encode(digest.finalize())
}

pub fn has_visible_mention(markdown: &str, handle: &str) -> bool {
    let mention = if handle.starts_with('@') { handle.to_owned() } else { format!("@{handle}") };
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &Options::default());
    let mut stack = root.children().collect::<Vec<_>>();
    while let Some(node) = stack.pop() {
        let data = node.data.borrow();
        if matches!(
            data.value,
            NodeValue::BlockQuote
                | NodeValue::Code(_)
                | NodeValue::CodeBlock(_)
                | NodeValue::HtmlInline(_)
                | NodeValue::HtmlBlock(_)
        ) {
            continue;
        }
        if let NodeValue::Text(text) = &data.value
            && contains_exact_mention(text, &mention)
        {
            return true;
        }
        drop(data);
        stack.extend(node.children());
    }
    false
}

fn contains_exact_mention(text: &str, mention: &str) -> bool {
    text.match_indices(mention).any(|(start, matched)| {
        let end = start + matched.len();
        let before = text[..start].chars().next_back();
        let after = text[end..].chars().next();
        !before.is_some_and(is_handle_character) && !after.is_some_and(is_handle_character)
    })
}

fn is_handle_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
}

#[derive(Debug, Deserialize)]
struct Payload {
    action: Option<String>,
    repository: Option<Repository>,
    sender: Option<Actor>,
    issue: Option<Issue>,
    pull_request: Option<PullRequest>,
    comment: Option<Comment>,
    review: Option<Review>,
    thread: Option<ReviewThread>,
}

#[derive(Debug, Deserialize)]
struct Repository {
    node_id: String,
    full_name: String,
}

#[derive(Debug, Deserialize)]
struct Actor {
    login: String,
    node_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Issue {
    node_id: String,
    number: u64,
    state: String,
    updated_at: Option<String>,
    body: Option<String>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct PullRequest {
    node_id: String,
    number: u64,
    state: String,
    updated_at: Option<String>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Comment {
    id: u64,
    node_id: String,
    body: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Review {
    node_id: String,
    body: Option<String>,
    submitted_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ReviewThread {
    node_id: String,
    updated_at: Option<String>,
}
