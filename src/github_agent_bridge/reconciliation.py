"""Canonical GitHub reconciliation for one Issue binding.

Reconciliation repairs transport gaps.  It emits only changed object refs and
lifecycles; the Agent remains responsible for reading and understanding the
canonical GitHub content.
"""

from __future__ import annotations

from collections.abc import Callable, Set
from dataclasses import dataclass
import hashlib
import json
import time
from typing import Protocol

from github_agent_bridge.github_api import (
    GitHubApiError,
    IssueCommentSnapshot,
    IssueReference,
    PullRequestCanonicalState,
    PullRequestCommentSnapshot,
    PullRequestReference,
    PullRequestReviewSnapshot,
    PullRequestReviewThreadSnapshot,
    PullRequestSnapshot,
    RepositoryPermission,
)
from github_agent_bridge.store import (
    Binding,
    EventEnvelope,
    LeaseToken,
    StateConflict,
    StoredEvent,
    SurfaceKind,
    SurfaceRoute,
    TransportStore,
)


class GitHubReconciliationAuthority(Protocol):
    async def issue_reference(
        self, repository_full_name: str, issue_number: int
    ) -> IssueReference: ...

    async def current_associated_pull_request(
        self, repository_full_name: str, issue_number: int
    ) -> PullRequestReference | None: ...

    async def issue_comments(
        self,
        repository_full_name: str,
        issue_number: int,
        *,
        self_logins: Set[str] = frozenset(),
    ) -> tuple[IssueCommentSnapshot, ...]: ...

    async def pull_request_state(
        self,
        reference: PullRequestReference,
        *,
        self_logins: Set[str] = frozenset(),
    ) -> PullRequestCanonicalState: ...

    async def repository_permission(
        self, repository_full_name: str, actor_login: str
    ) -> RepositoryPermission: ...


@dataclass(frozen=True, slots=True)
class ReconciliationReport:
    binding_id: str
    issue_event_created: bool
    comment_events_created: int
    permissions_resolved: int
    comments_seen: int
    current_pr_node_id: str | None
    pull_request_events_created: int
    pull_request_objects_seen: int


class GitHubReconciler:
    def __init__(
        self,
        authority: GitHubReconciliationAuthority,
        store: TransportStore,
        owner_token: LeaseToken,
        *,
        self_logins: Set[str] = frozenset(),
        clock: Callable[[], float] = time.time,
        quiet_window_seconds: float = 30.0,
    ) -> None:
        if quiet_window_seconds <= 0:
            raise ValueError("quiet_window_seconds must be positive")
        self._authority = authority
        self._store = store
        self._owner_token = owner_token
        self._self_logins = self_logins
        self._clock = clock
        self._quiet_window_seconds = quiet_window_seconds

    async def reconcile_binding(self, binding: Binding) -> ReconciliationReport:
        # Complete the remote read before mutating local projections. An
        # unavailable PR sub-connection must not create a partial canonical
        # checkpoint that could turn unseen objects into tombstones.
        issue = await self._authority.issue_reference(
            binding.repository_full_name, binding.issue_number
        )
        _require_same_issue(binding, issue)
        previous_route = await self._store.current_pr_route(binding.binding_id)
        current_pr = await self._authority.current_associated_pull_request(
            binding.repository_full_name, binding.issue_number
        )
        pr_state = (
            None
            if current_pr is None
            else await self._authority.pull_request_state(
                current_pr, self_logins=self._self_logins
            )
        )
        if pr_state is not None:
            _require_same_pull_request(current_pr, pr_state.pull_request)
        snapshots = await self._authority.issue_comments(
            binding.repository_full_name,
            binding.issue_number,
            self_logins=self._self_logins,
        )

        issue_event_created = await self._reconcile_issue(binding, issue)
        route = None if current_pr is None else _surface_route(binding, current_pr)
        await self._store.replace_current_pr_route(
            self._owner_token, binding.binding_id, route
        )
        if previous_route is not None and (
            route is None or route.surface_node_id != previous_route.surface_node_id
        ):
            await self._store.supersede_pending_events_for_surface(
                self._owner_token,
                binding.binding_id,
                previous_route.surface_node_id,
            )
        comment_events_created = await self._reconcile_comments(binding, snapshots)
        pull_request_events_created = (
            0
            if pr_state is None
            else await self._reconcile_pull_request(binding, pr_state)
        )
        permissions_resolved = await self.resolve_pending_permissions(binding)
        return ReconciliationReport(
            binding_id=binding.binding_id,
            issue_event_created=issue_event_created,
            comment_events_created=comment_events_created,
            permissions_resolved=permissions_resolved,
            comments_seen=len(snapshots),
            current_pr_node_id=None if current_pr is None else current_pr.pr_node_id,
            pull_request_events_created=pull_request_events_created,
            pull_request_objects_seen=(
                0 if pr_state is None else _pull_request_object_count(pr_state)
            ),
        )

    async def resolve_pending_permissions(self, binding: Binding) -> int:
        resolved = 0
        pending = await self._store.pending_events(
            self._owner_token, binding.binding_id, limit=1000
        )
        for event in pending:
            if (
                not event.mention_detected
                or event.permission_role is not None
                or event.actor_login is None
            ):
                continue
            try:
                permission = await self._authority.repository_permission(
                    binding.repository_full_name, event.actor_login
                )
            except GitHubApiError:
                # Unavailable/unknown permission is deliberately ordinary. A
                # later reconciliation may resolve it without blocking quiet
                # delivery of the attributed message ref.
                continue
            await self._store.resolve_event_permission(
                self._owner_token,
                event.event_id,
                permission.scheduling_role,
            )
            resolved += 1
        return resolved

    async def _reconcile_issue(
        self, binding: Binding, issue: IssueReference
    ) -> bool:
        previous = await self._store.latest_event_for_object(
            self._owner_token, binding.binding_id, binding.issue_node_id
        )
        state = issue.state.lower()
        if previous is not None and (
            previous.body_digest == issue.content_digest
            and _issue_action_matches_state(previous.action, state)
        ):
            return False
        if previous is None:
            action = "opened" if state == "open" else "closed"
        elif state == "closed" and previous.action != "closed":
            action = "closed"
        elif state == "open" and previous.action == "closed":
            action = "reopened"
        else:
            action = "edited"
        envelope = EventEnvelope(
            event_key=_reconciliation_key(
                binding.binding_id,
                binding.issue_node_id,
                issue.updated_at,
                action,
                issue.content_digest,
            ),
            binding_id=binding.binding_id,
            event_name="issues",
            action=action,
            object_node_id=binding.issue_node_id,
            surface_kind="issue",
            surface_node_id=binding.issue_node_id,
            object_version=issue.updated_at,
            body_digest=issue.content_digest,
            canonical_url=issue.issue_url,
            observed_at=self._clock(),
        )
        _, created = await self._ingest_and_schedule(envelope)
        return created

    async def _reconcile_comments(
        self,
        binding: Binding,
        snapshots: tuple[IssueCommentSnapshot, ...],
    ) -> int:
        previous_events = {
            event.object_node_id: event
            for event in await self._store.latest_events_for_surface(
                self._owner_token,
                binding.binding_id,
                event_name="issue_comment",
                surface_node_id=binding.issue_node_id,
            )
        }
        created_count = 0
        seen: set[str] = set()
        for snapshot in snapshots:
            if snapshot.object_node_id in seen:
                raise StateConflict("canonical reconciliation returned duplicate comment")
            seen.add(snapshot.object_node_id)
            action = _comment_action(previous_events.get(snapshot.object_node_id), snapshot)
            if action is None:
                continue
            envelope = EventEnvelope(
                event_key=_reconciliation_key(
                    binding.binding_id,
                    snapshot.object_node_id,
                    snapshot.object_version,
                    action,
                    snapshot.body_digest,
                ),
                binding_id=binding.binding_id,
                event_name="issue_comment",
                action=action,
                object_node_id=snapshot.object_node_id,
                surface_kind="issue",
                surface_node_id=binding.issue_node_id,
                object_version=snapshot.object_version,
                body_digest=snapshot.body_digest,
                canonical_url=snapshot.canonical_url,
                observed_at=self._clock(),
                actor_login=snapshot.author_login,
                author_association=snapshot.author_association,
                mention_detected=snapshot.mention_detected,
                wake_eligible=snapshot.wake_eligible,
            )
            _, created = await self._ingest_and_schedule(envelope)
            created_count += int(created)

        for node_id, previous in previous_events.items():
            if node_id in seen or previous.action == "deleted":
                continue
            envelope = EventEnvelope(
                event_key=_reconciliation_key(
                    binding.binding_id,
                    node_id,
                    f"missing-after-event:{previous.event_id}",
                    "deleted",
                    previous.body_digest,
                ),
                binding_id=binding.binding_id,
                event_name="issue_comment",
                action="deleted",
                object_node_id=node_id,
                surface_kind="issue",
                surface_node_id=binding.issue_node_id,
                object_version=f"missing-after-event:{previous.event_id}",
                body_digest=previous.body_digest,
                canonical_url=previous.canonical_url,
                observed_at=self._clock(),
                wake_eligible=True,
            )
            _, created = await self._ingest_and_schedule(envelope)
            created_count += int(created)
        return created_count

    async def _reconcile_pull_request(
        self,
        binding: Binding,
        state: PullRequestCanonicalState,
    ) -> int:
        pull_request = state.pull_request
        surface_node_id = pull_request.reference.pr_node_id
        created_count = await self._reconcile_pull_request_object(
            binding, pull_request
        )
        created_count += await self._reconcile_pull_request_objects(
            binding,
            surface_node_id=surface_node_id,
            event_name="issue_comment",
            snapshots=state.conversation_comments,
            action_for=_comment_action,
        )
        created_count += await self._reconcile_pull_request_objects(
            binding,
            surface_node_id=surface_node_id,
            event_name="pull_request_review",
            snapshots=state.reviews,
            action_for=_review_action,
        )
        created_count += await self._reconcile_pull_request_objects(
            binding,
            surface_node_id=surface_node_id,
            event_name="pull_request_review_comment",
            snapshots=state.review_comments,
            action_for=_comment_action,
        )
        created_count += await self._reconcile_review_threads(
            binding,
            surface_node_id=surface_node_id,
            snapshots=state.review_threads,
        )
        return created_count

    async def _reconcile_pull_request_object(
        self, binding: Binding, snapshot: PullRequestSnapshot
    ) -> int:
        previous = await self._store.latest_event_for_object(
            self._owner_token,
            binding.binding_id,
            snapshot.reference.pr_node_id,
        )
        action = _pull_request_action(previous, snapshot)
        if action is None:
            return 0
        envelope = EventEnvelope(
            event_key=_reconciliation_key(
                binding.binding_id,
                snapshot.reference.pr_node_id,
                snapshot.object_version,
                action,
                snapshot.body_digest,
            ),
            binding_id=binding.binding_id,
            event_name="pull_request",
            action=action,
            object_node_id=snapshot.reference.pr_node_id,
            surface_kind="pull_request",
            surface_node_id=snapshot.reference.pr_node_id,
            object_version=snapshot.object_version,
            body_digest=snapshot.body_digest,
            canonical_url=snapshot.reference.pr_url,
            observed_at=self._clock(),
            actor_node_id=snapshot.actor_node_id,
            actor_login=snapshot.actor_login,
            author_association=snapshot.author_association,
            mention_detected=snapshot.mention_detected,
            wake_eligible=snapshot.wake_eligible,
        )
        _, created = await self._ingest_and_schedule(envelope)
        return int(created)

    async def _reconcile_pull_request_objects(
        self,
        binding: Binding,
        *,
        surface_node_id: str,
        event_name: str,
        snapshots: tuple[PullRequestCommentSnapshot | PullRequestReviewSnapshot, ...],
        action_for: Callable[
            [
                StoredEvent | None,
                PullRequestCommentSnapshot | PullRequestReviewSnapshot,
            ],
            str | None,
        ],
    ) -> int:
        previous_events = {
            event.object_node_id: event
            for event in await self._store.latest_events_for_surface(
                self._owner_token,
                binding.binding_id,
                event_name=event_name,
                surface_node_id=surface_node_id,
            )
        }
        created_count = 0
        seen: set[str] = set()
        for snapshot in snapshots:
            if snapshot.object_node_id in seen:
                raise StateConflict(
                    f"canonical reconciliation returned duplicate {event_name} object"
                )
            seen.add(snapshot.object_node_id)
            action = action_for(previous_events.get(snapshot.object_node_id), snapshot)
            if action is None:
                continue
            envelope = EventEnvelope(
                event_key=_reconciliation_key(
                    binding.binding_id,
                    snapshot.object_node_id,
                    snapshot.object_version,
                    action,
                    snapshot.body_digest,
                ),
                binding_id=binding.binding_id,
                event_name=event_name,
                action=action,
                object_node_id=snapshot.object_node_id,
                surface_kind="pull_request",
                surface_node_id=surface_node_id,
                object_version=snapshot.object_version,
                body_digest=snapshot.body_digest,
                canonical_url=snapshot.canonical_url,
                observed_at=self._clock(),
                actor_node_id=snapshot.actor_node_id,
                actor_login=snapshot.actor_login,
                author_association=snapshot.author_association,
                mention_detected=snapshot.mention_detected,
                wake_eligible=snapshot.wake_eligible,
            )
            _, created = await self._ingest_and_schedule(envelope)
            created_count += int(created)

        created_count += await self._reconcile_missing_pull_request_objects(
            binding,
            surface_node_id=surface_node_id,
            event_name=event_name,
            previous_events=previous_events,
            seen=seen,
        )
        return created_count

    async def _reconcile_review_threads(
        self,
        binding: Binding,
        *,
        surface_node_id: str,
        snapshots: tuple[PullRequestReviewThreadSnapshot, ...],
    ) -> int:
        event_name = "pull_request_review_thread"
        previous_events = {
            event.object_node_id: event
            for event in await self._store.latest_events_for_surface(
                self._owner_token,
                binding.binding_id,
                event_name=event_name,
                surface_node_id=surface_node_id,
            )
        }
        created_count = 0
        seen: set[str] = set()
        for snapshot in snapshots:
            if snapshot.object_node_id in seen:
                raise StateConflict(
                    "canonical reconciliation returned duplicate review thread"
                )
            seen.add(snapshot.object_node_id)
            previous = previous_events.get(snapshot.object_node_id)
            action = _review_thread_action(previous, snapshot)
            if action is None:
                continue
            envelope = EventEnvelope(
                event_key=_reconciliation_key(
                    binding.binding_id,
                    snapshot.object_node_id,
                    snapshot.object_version,
                    action,
                    snapshot.body_digest,
                ),
                binding_id=binding.binding_id,
                event_name=event_name,
                action=action,
                object_node_id=snapshot.object_node_id,
                surface_kind="pull_request",
                surface_node_id=surface_node_id,
                object_version=snapshot.object_version,
                body_digest=snapshot.body_digest,
                canonical_url=snapshot.canonical_url,
                observed_at=self._clock(),
            )
            _, created = await self._ingest_and_schedule(envelope)
            created_count += int(created)

        created_count += await self._reconcile_missing_pull_request_objects(
            binding,
            surface_node_id=surface_node_id,
            event_name=event_name,
            previous_events=previous_events,
            seen=seen,
        )
        return created_count

    async def _reconcile_missing_pull_request_objects(
        self,
        binding: Binding,
        *,
        surface_node_id: str,
        event_name: str,
        previous_events: dict[str, StoredEvent],
        seen: set[str],
    ) -> int:
        created_count = 0
        for node_id, previous in previous_events.items():
            if node_id in seen or previous.action == "deleted":
                continue
            version = f"missing-after-event:{previous.event_id}"
            envelope = EventEnvelope(
                event_key=_reconciliation_key(
                    binding.binding_id,
                    node_id,
                    version,
                    "deleted",
                    previous.body_digest,
                ),
                binding_id=binding.binding_id,
                event_name=event_name,
                action="deleted",
                object_node_id=node_id,
                surface_kind="pull_request",
                surface_node_id=surface_node_id,
                object_version=version,
                body_digest=previous.body_digest,
                canonical_url=previous.canonical_url,
                observed_at=self._clock(),
                actor_node_id=previous.actor_node_id,
                actor_login=previous.actor_login,
                author_association=previous.author_association,
                wake_eligible=previous.wake_eligible,
            )
            _, created = await self._ingest_and_schedule(envelope)
            created_count += int(created)
        return created_count

    async def _ingest_and_schedule(
        self, envelope: EventEnvelope
    ) -> tuple[StoredEvent, bool]:
        stored, created = await self._store.ingest_event(
            self._owner_token, envelope
        )
        await self._store.schedule_event(
            self._owner_token,
            stored.event_id,
            quiet_window_seconds=self._quiet_window_seconds,
            received_at=stored.observed_at,
        )
        return stored, created


def _require_same_issue(binding: Binding, issue: IssueReference) -> None:
    expected = (
        binding.repository_node_id,
        binding.repository_full_name,
        binding.issue_node_id,
        binding.issue_number,
        binding.issue_url,
    )
    actual = (
        issue.repository_node_id,
        issue.repository_full_name,
        issue.issue_node_id,
        issue.issue_number,
        issue.issue_url,
    )
    if actual != expected:
        raise StateConflict("canonical Issue identity no longer matches binding")


def _require_same_pull_request(
    reference: PullRequestReference, snapshot: PullRequestSnapshot
) -> None:
    expected = (
        reference.repository_node_id,
        reference.repository_full_name,
        reference.pr_node_id,
        reference.pr_number,
        reference.pr_url,
    )
    actual = (
        snapshot.reference.repository_node_id,
        snapshot.reference.repository_full_name,
        snapshot.reference.pr_node_id,
        snapshot.reference.pr_number,
        snapshot.reference.pr_url,
    )
    if actual != expected:
        raise StateConflict(
            "canonical pull request identity no longer matches association"
        )


def _surface_route(
    binding: Binding, pull_request: PullRequestReference
) -> SurfaceRoute:
    digest = _reconciliation_key(
        binding.binding_id,
        pull_request.pr_node_id,
        pull_request.repository_node_id,
        str(pull_request.pr_number),
        pull_request.pr_url,
    )
    return SurfaceRoute(
        binding_id=binding.binding_id,
        surface_kind=SurfaceKind.PULL_REQUEST,
        repository_node_id=pull_request.repository_node_id,
        repository_full_name=pull_request.repository_full_name,
        surface_node_id=pull_request.pr_node_id,
        surface_number=pull_request.pr_number,
        canonical_url=pull_request.pr_url,
        association_version=digest,
    )


def _issue_action_matches_state(action: str, state: str) -> bool:
    if state == "closed":
        return action == "closed"
    return action in {"opened", "edited", "reopened"}


def _comment_action(
    previous: StoredEvent | None,
    snapshot: IssueCommentSnapshot | PullRequestCommentSnapshot,
) -> str | None:
    if previous is None or previous.action == "deleted":
        return "minimized" if snapshot.is_minimized else "created"
    if snapshot.is_minimized:
        return None if previous.action == "minimized" else "minimized"
    if previous.action == "minimized":
        return "unminimized"
    if (
        previous.object_version != snapshot.object_version
        or previous.body_digest != snapshot.body_digest
    ):
        return "edited"
    return None


def _review_action(
    previous: StoredEvent | None,
    snapshot: PullRequestCommentSnapshot | PullRequestReviewSnapshot,
) -> str | None:
    if not isinstance(snapshot, PullRequestReviewSnapshot):
        raise TypeError("review action requires a review snapshot")
    if previous is None or previous.action == "deleted":
        if snapshot.state == "DISMISSED":
            return "dismissed"
        return "minimized" if snapshot.is_minimized else "submitted"
    if snapshot.state == "DISMISSED":
        return None if previous.action == "dismissed" else "dismissed"
    if snapshot.is_minimized:
        return None if previous.action == "minimized" else "minimized"
    if previous.action == "minimized":
        return "unminimized"
    if (
        previous.object_version != snapshot.object_version
        or previous.body_digest != snapshot.body_digest
        or previous.action == "dismissed"
    ):
        return "edited"
    return None


def _review_thread_action(
    previous: StoredEvent | None,
    snapshot: PullRequestReviewThreadSnapshot,
) -> str | None:
    expected = "resolved" if snapshot.is_resolved else "unresolved"
    if previous is None or previous.action == "deleted":
        return expected
    return None if previous.action == expected else expected


def _pull_request_action(
    previous: StoredEvent | None, snapshot: PullRequestSnapshot
) -> str | None:
    state = snapshot.reference.state
    if previous is None or previous.action == "deleted":
        return "opened" if state == "OPEN" else "closed"
    previous_facts = _pull_request_version_facts(previous.object_version)
    if previous_facts is not None:
        previous_state, previous_draft, previous_head = previous_facts
        if state in {"CLOSED", "MERGED"} and previous_state == "OPEN":
            return "closed"
        if state == "OPEN" and previous_state in {"CLOSED", "MERGED"}:
            return "reopened"
        if snapshot.reference.is_draft != previous_draft:
            return (
                "converted_to_draft"
                if snapshot.reference.is_draft
                else "ready_for_review"
            )
        if snapshot.head_ref_oid != previous_head:
            return "synchronize"
    elif state in {"CLOSED", "MERGED"} and previous.action != "closed":
        return "closed"
    elif state == "OPEN" and previous.action == "closed":
        return "reopened"
    if (
        previous.object_version != snapshot.object_version
        or previous.body_digest != snapshot.body_digest
    ):
        return "edited"
    return None


def _pull_request_version_facts(value: str) -> tuple[str, bool, str] | None:
    try:
        decoded = json.loads(value)
    except json.JSONDecodeError:
        return None
    if not isinstance(decoded, dict):
        return None
    state = decoded.get("state")
    draft = decoded.get("draft")
    head = decoded.get("head")
    if (
        not isinstance(state, str)
        or not isinstance(draft, bool)
        or not isinstance(head, str)
        or not head
    ):
        return None
    return state, draft, head


def _pull_request_object_count(state: PullRequestCanonicalState) -> int:
    return (
        1
        + len(state.conversation_comments)
        + len(state.reviews)
        + len(state.review_comments)
        + len(state.review_threads)
    )


def _reconciliation_key(*parts: str) -> str:
    encoded = json.dumps(
        parts, ensure_ascii=True, separators=(",", ":")
    ).encode("utf-8")
    return "github-reconcile:" + hashlib.sha256(encoded).hexdigest()
