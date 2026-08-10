"""Verified GitHub webhook normalization without semantic interpretation."""

from __future__ import annotations

from collections.abc import Mapping, Set
from dataclasses import dataclass
from enum import StrEnum
import hashlib
import hmac
import json
import re
from typing import Any

from github_agent_bridge.store import (
    EventEnvelope,
    TRUSTED_URGENT_PERMISSION_ROLES,
)


ISSUE_ACTIONS = frozenset({"opened", "edited", "deleted", "closed", "reopened"})
ISSUE_COMMENT_ACTIONS = frozenset({"created", "edited", "deleted"})
PULL_REQUEST_ACTIONS = frozenset(
    {
        "opened",
        "edited",
        "synchronize",
        "closed",
        "reopened",
        "converted_to_draft",
        "ready_for_review",
    }
)
PULL_REQUEST_REVIEW_ACTIONS = frozenset({"submitted", "edited", "dismissed"})
PULL_REQUEST_REVIEW_COMMENT_ACTIONS = frozenset(
    {"created", "edited", "deleted"}
)
PULL_REQUEST_REVIEW_THREAD_ACTIONS = frozenset({"resolved", "unresolved"})


class WebhookError(ValueError):
    """Base error for a rejected webhook delivery."""


class SignatureError(WebhookError):
    """The delivery does not authenticate against the configured secret."""


class HeaderError(WebhookError):
    """A required GitHub webhook header is absent or malformed."""


class PayloadError(WebhookError):
    """A supported event has a malformed payload."""


class UnsupportedEvent(WebhookError):
    """The event or action is outside the current bootstrap contract."""


class UnsupportedSurface(WebhookError):
    """The event belongs to a surface not enabled in the bootstrap."""


class ObjectLifecycle(StrEnum):
    ACTIVE = "active"
    EDITED = "edited"
    DELETED = "deleted"
    CLOSED = "closed"
    REOPENED = "reopened"
    DRAFT = "draft"
    READY = "ready"
    SYNCHRONIZED = "synchronized"
    SUBMITTED = "submitted"
    DISMISSED = "dismissed"
    RESOLVED = "resolved"
    UNRESOLVED = "unresolved"


@dataclass(frozen=True, slots=True)
class WebhookPing:
    delivery_id: str
    hook_id: int
    zen_digest: str


@dataclass(frozen=True, slots=True)
class NormalizedGitHubEvent:
    delivery_id: str
    event_name: str
    action: str
    repository_node_id: str
    repository_full_name: str
    surface_kind: str
    surface_node_id: str
    surface_number: int
    object_kind: str
    object_node_id: str
    canonical_url: str
    object_version: str
    body_digest: str
    actor_node_id: str | None
    actor_login: str | None
    author_association: str | None
    permission_role: str | None
    mention_detected: bool
    urgent: bool
    wake_eligible: bool
    lifecycle: ObjectLifecycle

    @property
    def event_key(self) -> str:
        return f"github-delivery:{self.delivery_id}"

    def to_event_envelope(
        self,
        *,
        binding_id: str,
        observed_at: float,
    ) -> EventEnvelope:
        return EventEnvelope(
            event_key=self.event_key,
            delivery_id=self.delivery_id,
            binding_id=binding_id,
            event_name=self.event_name,
            action=self.action,
            object_node_id=self.object_node_id,
            surface_kind=self.surface_kind,
            surface_node_id=self.surface_node_id,
            object_version=self.object_version,
            body_digest=self.body_digest,
            canonical_url=self.canonical_url,
            observed_at=observed_at,
            actor_node_id=self.actor_node_id,
            actor_login=self.actor_login,
            author_association=self.author_association,
            permission_role=self.permission_role,
            mention_detected=self.mention_detected,
            urgent=self.urgent,
            wake_eligible=self.wake_eligible,
        )

    @property
    def issue_node_id(self) -> str:
        if self.surface_kind != "issue":
            raise AttributeError("event surface is not an Issue")
        return self.surface_node_id

    @property
    def issue_number(self) -> int:
        if self.surface_kind != "issue":
            raise AttributeError("event surface is not an Issue")
        return self.surface_number


def parse_verified_webhook(
    raw_body: bytes,
    headers: Mapping[str, str],
    *,
    webhook_secret: bytes,
    permission_role: str | None = None,
    self_logins: Set[str] = frozenset(),
) -> NormalizedGitHubEvent | WebhookPing:
    """Authenticate raw bytes, then normalize a bounded event shape."""

    signature = _required_header(headers, "X-Hub-Signature-256")
    verify_signature(raw_body, signature, webhook_secret)
    delivery_id = _required_header(headers, "X-GitHub-Delivery")
    event_name = _required_header(headers, "X-GitHub-Event")
    payload = _parse_json_object(raw_body)

    if event_name == "ping":
        hook_id = payload.get("hook_id")
        zen = payload.get("zen")
        if not isinstance(hook_id, int) or isinstance(hook_id, bool):
            raise PayloadError("ping.hook_id must be an integer")
        if not isinstance(zen, str):
            raise PayloadError("ping.zen must be a string")
        return WebhookPing(
            delivery_id=delivery_id,
            hook_id=hook_id,
            zen_digest=_digest_text(zen),
        )

    action = _required_string(payload, "action", event_name)
    if event_name == "issues":
        if action not in ISSUE_ACTIONS:
            raise UnsupportedEvent(f"unsupported issues action: {action}")
        return _normalize_issue(
            payload,
            delivery_id=delivery_id,
            action=action,
            permission_role=permission_role,
            self_logins=self_logins,
        )
    if event_name == "issue_comment":
        if action not in ISSUE_COMMENT_ACTIONS:
            raise UnsupportedEvent(f"unsupported issue_comment action: {action}")
        return _normalize_issue_comment(
            payload,
            delivery_id=delivery_id,
            action=action,
            permission_role=permission_role,
            self_logins=self_logins,
        )
    if event_name == "pull_request":
        if action not in PULL_REQUEST_ACTIONS:
            raise UnsupportedEvent(f"unsupported pull_request action: {action}")
        return _normalize_pull_request(
            payload,
            delivery_id=delivery_id,
            action=action,
            permission_role=permission_role,
            self_logins=self_logins,
        )
    if event_name == "pull_request_review":
        if action not in PULL_REQUEST_REVIEW_ACTIONS:
            raise UnsupportedEvent(f"unsupported pull_request_review action: {action}")
        return _normalize_pull_request_child(
            payload,
            delivery_id=delivery_id,
            event_name=event_name,
            action=action,
            child_key="review",
            permission_role=permission_role,
            self_logins=self_logins,
        )
    if event_name == "pull_request_review_comment":
        if action not in PULL_REQUEST_REVIEW_COMMENT_ACTIONS:
            raise UnsupportedEvent(
                f"unsupported pull_request_review_comment action: {action}"
            )
        return _normalize_pull_request_child(
            payload,
            delivery_id=delivery_id,
            event_name=event_name,
            action=action,
            child_key="comment",
            permission_role=permission_role,
            self_logins=self_logins,
        )
    if event_name == "pull_request_review_thread":
        if action not in PULL_REQUEST_REVIEW_THREAD_ACTIONS:
            raise UnsupportedEvent(
                f"unsupported pull_request_review_thread action: {action}"
            )
        return _normalize_review_thread(
            payload,
            delivery_id=delivery_id,
            action=action,
            self_logins=self_logins,
        )
    raise UnsupportedEvent(f"unsupported GitHub event: {event_name}")


def verify_signature(raw_body: bytes, signature: str, webhook_secret: bytes) -> None:
    if not webhook_secret:
        raise SignatureError("webhook secret must not be empty")
    if not re.fullmatch(r"sha256=[0-9a-fA-F]{64}", signature):
        raise SignatureError("X-Hub-Signature-256 is malformed")
    expected = "sha256=" + hmac.new(
        webhook_secret, raw_body, hashlib.sha256
    ).hexdigest()
    if not hmac.compare_digest(signature.lower(), expected):
        raise SignatureError("webhook signature does not match")


def has_visible_agent_mention(body: str) -> bool:
    """Detect exact lowercase @agent outside non-visible Markdown regions."""

    visible = _strip_html_comments(body)
    visible = _strip_html_code_regions(visible)
    visible = _strip_fenced_code(visible)
    visible = _strip_indented_code(visible)
    visible = _strip_inline_code(visible)
    visible = re.sub(r"<(?:(?:https?|mailto):)[^>]*>", " ", visible)
    visible = re.sub(r"\]\([^\n)]*\)", "]", visible)
    visible = re.sub(r"\b(?:https?://|mailto:)\S+", " ", visible)
    visible = re.sub(r"<[^>]+>", " ", visible)
    return re.search(r"(?<![A-Za-z0-9_.-])@agent(?![A-Za-z0-9_.-])", visible) is not None


def _normalize_issue(
    payload: dict[str, Any],
    *,
    delivery_id: str,
    action: str,
    permission_role: str | None,
    self_logins: Set[str],
) -> NormalizedGitHubEvent:
    repository, issue, sender = _common_objects(payload)
    body = issue.get("body")
    title = issue.get("title")
    state = issue.get("state")
    if body is not None and not isinstance(body, str):
        raise PayloadError("issue.body must be a string or null")
    if not isinstance(title, str) or not isinstance(state, str):
        raise PayloadError("issue title/state must be strings")
    actor_node_id, actor_login = _actor(sender)
    self_origin = _is_self_origin(actor_login, self_logins)
    mention = (
        action != "deleted"
        and not self_origin
        and has_visible_agent_mention(body or "")
    )
    lifecycle = {
        "opened": ObjectLifecycle.ACTIVE,
        "edited": ObjectLifecycle.EDITED,
        "deleted": ObjectLifecycle.DELETED,
        "closed": ObjectLifecycle.CLOSED,
        "reopened": ObjectLifecycle.REOPENED,
    }[action]
    return NormalizedGitHubEvent(
        delivery_id=delivery_id,
        event_name="issues",
        action=action,
        repository_node_id=_required_string(repository, "node_id", "repository"),
        repository_full_name=_required_string(
            repository, "full_name", "repository"
        ),
        surface_kind="issue",
        surface_node_id=_required_string(issue, "node_id", "issue"),
        surface_number=_required_positive_int(issue, "number", "issue"),
        object_kind="issue",
        object_node_id=_required_string(issue, "node_id", "issue"),
        canonical_url=_required_string(issue, "html_url", "issue"),
        object_version=_object_version(issue, delivery_id),
        body_digest=digest_issue_content(title=title, body=body, state=state),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=_optional_string(issue.get("author_association")),
        permission_role=permission_role,
        mention_detected=mention,
        urgent=_is_urgent(mention, permission_role),
        wake_eligible=not self_origin,
        lifecycle=lifecycle,
    )


def _normalize_issue_comment(
    payload: dict[str, Any],
    *,
    delivery_id: str,
    action: str,
    permission_role: str | None,
    self_logins: Set[str],
) -> NormalizedGitHubEvent:
    repository, issue, sender = _common_objects(payload)
    surface_kind = "pull_request" if "pull_request" in issue else "issue"
    comment = _required_object(payload, "comment", "issue_comment")
    body = comment.get("body")
    if body is not None and not isinstance(body, str):
        raise PayloadError("comment.body must be a string or null")
    actor_node_id, actor_login = _actor(sender)
    self_origin = _is_self_origin(actor_login, self_logins)
    mention = (
        action != "deleted"
        and not self_origin
        and has_visible_agent_mention(body or "")
    )
    lifecycle = {
        "created": ObjectLifecycle.ACTIVE,
        "edited": ObjectLifecycle.EDITED,
        "deleted": ObjectLifecycle.DELETED,
    }[action]
    return NormalizedGitHubEvent(
        delivery_id=delivery_id,
        event_name="issue_comment",
        action=action,
        repository_node_id=_required_string(repository, "node_id", "repository"),
        repository_full_name=_required_string(
            repository, "full_name", "repository"
        ),
        surface_kind=surface_kind,
        surface_node_id=_required_string(issue, "node_id", "issue"),
        surface_number=_required_positive_int(issue, "number", "issue"),
        object_kind="issue_comment",
        object_node_id=_required_string(comment, "node_id", "comment"),
        canonical_url=_required_string(comment, "html_url", "comment"),
        object_version=_object_version(comment, delivery_id),
        body_digest=digest_comment_body(body or ""),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=_optional_string(comment.get("author_association")),
        permission_role=permission_role,
        mention_detected=mention,
        urgent=_is_urgent(mention, permission_role),
        wake_eligible=not self_origin,
        lifecycle=lifecycle,
    )


def _normalize_pull_request(
    payload: dict[str, Any],
    *,
    delivery_id: str,
    action: str,
    permission_role: str | None,
    self_logins: Set[str],
) -> NormalizedGitHubEvent:
    repository = _required_object(payload, "repository", "payload")
    pull_request = _required_object(payload, "pull_request", "payload")
    sender_value = payload.get("sender")
    if sender_value is not None and not isinstance(sender_value, dict):
        raise PayloadError("payload.sender must be an object or null")
    body = pull_request.get("body")
    if body is not None and not isinstance(body, str):
        raise PayloadError("pull_request.body must be a string or null")
    title = _required_string(pull_request, "title", "pull_request")
    state = _required_string(pull_request, "state", "pull_request")
    draft = pull_request.get("draft")
    if not isinstance(draft, bool):
        raise PayloadError("pull_request.draft must be a boolean")
    actor_node_id, actor_login = _actor(sender_value)
    self_origin = _is_self_origin(actor_login, self_logins)
    mention = (
        action not in {"closed"}
        and not self_origin
        and has_visible_agent_mention(body or "")
    )
    lifecycle = {
        "opened": ObjectLifecycle.ACTIVE,
        "edited": ObjectLifecycle.EDITED,
        "synchronize": ObjectLifecycle.SYNCHRONIZED,
        "closed": ObjectLifecycle.CLOSED,
        "reopened": ObjectLifecycle.REOPENED,
        "converted_to_draft": ObjectLifecycle.DRAFT,
        "ready_for_review": ObjectLifecycle.READY,
    }[action]
    return NormalizedGitHubEvent(
        delivery_id=delivery_id,
        event_name="pull_request",
        action=action,
        repository_node_id=_required_string(repository, "node_id", "repository"),
        repository_full_name=_required_string(repository, "full_name", "repository"),
        surface_kind="pull_request",
        surface_node_id=_required_string(pull_request, "node_id", "pull_request"),
        surface_number=_required_positive_int(pull_request, "number", "pull_request"),
        object_kind="pull_request",
        object_node_id=_required_string(pull_request, "node_id", "pull_request"),
        canonical_url=_required_string(pull_request, "html_url", "pull_request"),
        object_version=_object_version(pull_request, delivery_id),
        body_digest=_digest_json(
            {"body": body, "draft": draft, "state": state, "title": title}
        ),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=_optional_string(
            pull_request.get("author_association")
        ),
        permission_role=permission_role,
        mention_detected=mention,
        urgent=_is_urgent(mention, permission_role),
        wake_eligible=not self_origin,
        lifecycle=lifecycle,
    )


def _normalize_pull_request_child(
    payload: dict[str, Any],
    *,
    delivery_id: str,
    event_name: str,
    action: str,
    child_key: str,
    permission_role: str | None,
    self_logins: Set[str],
) -> NormalizedGitHubEvent:
    repository = _required_object(payload, "repository", "payload")
    pull_request = _required_object(payload, "pull_request", "payload")
    child = _required_object(payload, child_key, event_name)
    sender_value = payload.get("sender")
    if sender_value is not None and not isinstance(sender_value, dict):
        raise PayloadError("payload.sender must be an object or null")
    body = child.get("body")
    if body is not None and not isinstance(body, str):
        raise PayloadError(f"{child_key}.body must be a string or null")
    actor_node_id, actor_login = _actor(sender_value)
    self_origin = _is_self_origin(actor_login, self_logins)
    mention = (
        action not in {"deleted", "dismissed"}
        and not self_origin
        and has_visible_agent_mention(body or "")
    )
    lifecycle = {
        "created": ObjectLifecycle.ACTIVE,
        "submitted": ObjectLifecycle.SUBMITTED,
        "edited": ObjectLifecycle.EDITED,
        "deleted": ObjectLifecycle.DELETED,
        "dismissed": ObjectLifecycle.DISMISSED,
    }[action]
    return NormalizedGitHubEvent(
        delivery_id=delivery_id,
        event_name=event_name,
        action=action,
        repository_node_id=_required_string(repository, "node_id", "repository"),
        repository_full_name=_required_string(repository, "full_name", "repository"),
        surface_kind="pull_request",
        surface_node_id=_required_string(pull_request, "node_id", "pull_request"),
        surface_number=_required_positive_int(pull_request, "number", "pull_request"),
        object_kind=event_name,
        object_node_id=_required_string(child, "node_id", child_key),
        canonical_url=_required_string(child, "html_url", child_key),
        object_version=_object_version(child, delivery_id),
        body_digest=digest_comment_body(body or ""),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=_optional_string(child.get("author_association")),
        permission_role=permission_role,
        mention_detected=mention,
        urgent=_is_urgent(mention, permission_role),
        wake_eligible=not self_origin,
        lifecycle=lifecycle,
    )


def _normalize_review_thread(
    payload: dict[str, Any],
    *,
    delivery_id: str,
    action: str,
    self_logins: Set[str],
) -> NormalizedGitHubEvent:
    repository = _required_object(payload, "repository", "payload")
    pull_request = _required_object(payload, "pull_request", "payload")
    thread = _required_object(payload, "thread", "pull_request_review_thread")
    sender_value = payload.get("sender")
    if sender_value is not None and not isinstance(sender_value, dict):
        raise PayloadError("payload.sender must be an object or null")
    actor_node_id, actor_login = _actor(sender_value)
    self_origin = _is_self_origin(actor_login, self_logins)
    lifecycle = (
        ObjectLifecycle.RESOLVED
        if action == "resolved"
        else ObjectLifecycle.UNRESOLVED
    )
    comments = thread.get("comments")
    if not isinstance(comments, list):
        raise PayloadError("thread.comments must be a list")
    comment_refs = []
    for comment in comments:
        if not isinstance(comment, dict):
            raise PayloadError("thread comment must be an object")
        comment_refs.append(_required_string(comment, "node_id", "thread comment"))
    return NormalizedGitHubEvent(
        delivery_id=delivery_id,
        event_name="pull_request_review_thread",
        action=action,
        repository_node_id=_required_string(repository, "node_id", "repository"),
        repository_full_name=_required_string(repository, "full_name", "repository"),
        surface_kind="pull_request",
        surface_node_id=_required_string(pull_request, "node_id", "pull_request"),
        surface_number=_required_positive_int(pull_request, "number", "pull_request"),
        object_kind="pull_request_review_thread",
        object_node_id=_required_string(thread, "node_id", "thread"),
        canonical_url=_required_string(pull_request, "html_url", "pull_request"),
        object_version=_object_version(thread, delivery_id),
        body_digest=_digest_json(
            {"action": action, "comment_node_ids": comment_refs}
        ),
        actor_node_id=actor_node_id,
        actor_login=actor_login,
        author_association=None,
        permission_role=None,
        mention_detected=False,
        urgent=False,
        wake_eligible=not self_origin,
        lifecycle=lifecycle,
    )


def _common_objects(
    payload: dict[str, Any],
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any] | None]:
    repository = _required_object(payload, "repository", "payload")
    issue = _required_object(payload, "issue", "payload")
    sender_value = payload.get("sender")
    if sender_value is not None and not isinstance(sender_value, dict):
        raise PayloadError("payload.sender must be an object or null")
    return repository, issue, sender_value


def _actor(sender: dict[str, Any] | None) -> tuple[str | None, str | None]:
    if sender is None:
        return None, None
    return (
        _optional_string(sender.get("node_id")),
        _optional_string(sender.get("login")),
    )


def _is_self_origin(actor_login: str | None, self_logins: Set[str]) -> bool:
    if actor_login is None:
        return False
    normalized = {login.casefold() for login in self_logins}
    return actor_login.casefold() in normalized


def _is_urgent(mention_detected: bool, permission_role: str | None) -> bool:
    return mention_detected and permission_role in TRUSTED_URGENT_PERMISSION_ROLES


def _object_version(value: dict[str, Any], delivery_id: str) -> str:
    for key in ("updated_at", "created_at"):
        candidate = value.get(key)
        if isinstance(candidate, str) and candidate:
            return candidate
    return f"delivery:{delivery_id}"


def _parse_json_object(raw_body: bytes) -> dict[str, Any]:
    try:
        value = json.loads(raw_body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PayloadError("webhook body is not valid UTF-8 JSON") from error
    if not isinstance(value, dict):
        raise PayloadError("webhook body must be a JSON object")
    return value


def _required_header(headers: Mapping[str, str], name: str) -> str:
    expected = name.casefold()
    for key, value in headers.items():
        if key.casefold() == expected and isinstance(value, str) and value:
            return value
    raise HeaderError(f"missing required header {name}")


def _required_object(
    value: dict[str, Any], key: str, owner: str
) -> dict[str, Any]:
    member = value.get(key)
    if not isinstance(member, dict):
        raise PayloadError(f"{owner}.{key} must be an object")
    return member


def _required_string(value: dict[str, Any], key: str, owner: str) -> str:
    member = value.get(key)
    if not isinstance(member, str) or not member:
        raise PayloadError(f"{owner}.{key} must be a non-empty string")
    return member


def _required_positive_int(value: dict[str, Any], key: str, owner: str) -> int:
    member = value.get(key)
    if not isinstance(member, int) or isinstance(member, bool) or member < 1:
        raise PayloadError(f"{owner}.{key} must be a positive integer")
    return member


def _optional_string(value: Any) -> str | None:
    return value if isinstance(value, str) and value else None


def _digest_text(value: str) -> str:
    return "sha256:" + hashlib.sha256(value.encode("utf-8")).hexdigest()


def digest_comment_body(body: str) -> str:
    return _digest_text(body)


def digest_issue_content(
    *, title: str, body: str | None, state: str
) -> str:
    return _digest_json({"body": body, "state": state, "title": title})


def _digest_json(value: Any) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
    return "sha256:" + hashlib.sha256(encoded).hexdigest()


def _strip_html_comments(value: str) -> str:
    value = re.sub(r"<!--.*?-->", " ", value, flags=re.DOTALL)
    return re.sub(r"<!--.*\Z", " ", value, flags=re.DOTALL)


def _strip_html_code_regions(value: str) -> str:
    for element in ("pre", "code"):
        value = re.sub(
            rf"<{element}\b[^>]*>.*?</{element}\s*>",
            " ",
            value,
            flags=re.DOTALL | re.IGNORECASE,
        )
    return value


def _strip_indented_code(value: str) -> str:
    pattern = re.compile(r"^(?:(?: {0,3}> ?)+)?(?: {4}|\t).*$")
    return "".join(
        "\n" if pattern.match(line.rstrip("\r\n")) else line
        for line in value.splitlines(keepends=True)
    )


def _strip_fenced_code(value: str) -> str:
    output: list[str] = []
    fence_character: str | None = None
    fence_length = 0
    fence_pattern = re.compile(r"^(?: {0,3}> ?)* {0,3}(`{3,}|~{3,})")
    for line in value.splitlines(keepends=True):
        match = fence_pattern.match(line)
        if fence_character is None:
            if match is None:
                output.append(line)
                continue
            run = match.group(1)
            fence_character = run[0]
            fence_length = len(run)
            output.append("\n" if line.endswith("\n") else "")
            continue
        if match is not None:
            run = match.group(1)
            if run[0] == fence_character and len(run) >= fence_length:
                fence_character = None
                fence_length = 0
        output.append("\n" if line.endswith("\n") else "")
    return "".join(output)


def _strip_inline_code(value: str) -> str:
    output: list[str] = []
    index = 0
    while index < len(value):
        if value[index] != "`":
            output.append(value[index])
            index += 1
            continue
        end_of_run = index
        while end_of_run < len(value) and value[end_of_run] == "`":
            end_of_run += 1
        delimiter = value[index:end_of_run]
        closing = value.find(delimiter, end_of_run)
        if closing == -1:
            output.append(delimiter)
            index = end_of_run
            continue
        output.append(" ")
        index = closing + len(delimiter)
    return "".join(output)
