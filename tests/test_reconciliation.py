from __future__ import annotations

import asyncio
from dataclasses import replace
import json
from pathlib import Path
import tempfile
import unittest

from github_agent_bridge.github_api import (
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
from github_agent_bridge.github_webhook import (
    digest_comment_body,
    digest_issue_content,
)
from github_agent_bridge.reconciliation import GitHubReconciler
from github_agent_bridge.store import (
    Binding,
    EventState,
    StateConflict,
    TransportStore,
)


def binding() -> Binding:
    return Binding(
        binding_id="binding-1",
        repository_node_id="R_repository",
        repository_full_name="owner/repository",
        issue_node_id="I_issue",
        issue_number=17,
        issue_url="https://github.example/owner/repository/issues/17",
        thread_address="opaque-thread",
        agent_identity="agent-bot",
        wrapper_identity="wrapper-bot",
        trusted_permission="triage",
        instruction_digest="sha256:instructions",
    )


def issue_reference() -> IssueReference:
    return IssueReference(
        repository_node_id="R_repository",
        repository_full_name="owner/repository",
        default_branch="main",
        issue_node_id="I_issue",
        issue_number=17,
        issue_url="https://github.example/owner/repository/issues/17",
        state="OPEN",
        updated_at="2026-08-10T12:00:00Z",
        content_digest=digest_issue_content(
            title="A bounded task", body="Discussion", state="open"
        ),
    )


def comment(
    node_id: str,
    body: str,
    *,
    version: str = "2026-08-10T12:00:00Z",
    author: str = "human",
    mention: bool = False,
    wake_eligible: bool = True,
    minimized: bool = False,
) -> IssueCommentSnapshot:
    return IssueCommentSnapshot(
        object_node_id=node_id,
        canonical_url=f"https://github.example/comments/{node_id}",
        object_version=version,
        body_digest=digest_comment_body(body),
        author_login=author,
        author_association="MEMBER",
        mention_detected=mention,
        wake_eligible=wake_eligible,
        is_minimized=minimized,
        minimized_reason="OUTDATED" if minimized else None,
    )


def pr_snapshot(
    reference: PullRequestReference,
    *,
    state: str = "OPEN",
    draft: bool = True,
    head: str = "a" * 40,
    version: str = "2026-08-10T12:10:00Z",
    body: str = "candidate body",
) -> PullRequestSnapshot:
    current = replace(reference, state=state, is_draft=draft)
    object_version = json.dumps(
        {
            "draft": draft,
            "head": head,
            "state": state,
            "updated_at": version,
        },
        separators=(",", ":"),
        sort_keys=True,
    )
    return PullRequestSnapshot(
        reference=current,
        object_version=object_version,
        body_digest=digest_comment_body(
            f"{body}|{state}|{int(draft)}|{head}"
        ),
        head_ref_oid=head,
        actor_node_id="U_author",
        actor_login="author",
        author_association="MEMBER",
        mention_detected=False,
        wake_eligible=True,
    )


def pr_comment(
    node_id: str,
    body: str,
    *,
    version: str = "2026-08-10T12:11:00Z",
    minimized: bool = False,
) -> PullRequestCommentSnapshot:
    return PullRequestCommentSnapshot(
        object_node_id=node_id,
        canonical_url=f"https://github.example/pr-comment/{node_id}",
        object_version=version,
        body_digest=digest_comment_body(body),
        actor_node_id="U_reviewer",
        actor_login="reviewer",
        author_association="MEMBER",
        mention_detected=False,
        wake_eligible=True,
        is_minimized=minimized,
        minimized_reason="OUTDATED" if minimized else None,
    )


def review(
    node_id: str,
    body: str,
    *,
    version: str = "2026-08-10T12:12:00Z",
    state: str = "CHANGES_REQUESTED",
    minimized: bool = False,
) -> PullRequestReviewSnapshot:
    comment_snapshot = pr_comment(
        node_id, body, version=version, minimized=minimized
    )
    return PullRequestReviewSnapshot(
        object_node_id=comment_snapshot.object_node_id,
        canonical_url=comment_snapshot.canonical_url,
        object_version=comment_snapshot.object_version,
        body_digest=comment_snapshot.body_digest,
        actor_node_id=comment_snapshot.actor_node_id,
        actor_login=comment_snapshot.actor_login,
        author_association=comment_snapshot.author_association,
        mention_detected=comment_snapshot.mention_detected,
        wake_eligible=comment_snapshot.wake_eligible,
        state=state,
        is_minimized=minimized,
        minimized_reason=comment_snapshot.minimized_reason,
    )


def thread(
    node_id: str, *, resolved: bool
) -> PullRequestReviewThreadSnapshot:
    digest = digest_comment_body(f"{node_id}:{int(resolved)}")
    return PullRequestReviewThreadSnapshot(
        object_node_id=node_id,
        canonical_url=f"https://github.example/thread/{node_id}",
        object_version="canonical:" + digest.removeprefix("sha256:"),
        body_digest=digest,
        is_resolved=resolved,
    )


class FakeAuthority:
    def __init__(self) -> None:
        self.issue = issue_reference()
        self.pull_request = PullRequestReference(
            repository_node_id="R_repository",
            repository_full_name="owner/repository",
            pr_node_id="PR_candidate",
            pr_number=23,
            pr_url="https://github.example/owner/repository/pull/23",
            state="OPEN",
            is_draft=True,
        )
        self.pr_state = PullRequestCanonicalState(
            pull_request=pr_snapshot(self.pull_request),
            conversation_comments=(),
            reviews=(),
            review_comments=(),
            review_threads=(),
        )
        self.comments = (
            comment("IC_ordinary", "ordinary discussion"),
            comment("IC_urgent", "@agent", mention=True),
            comment(
                "IC_self",
                "wrapper mirror",
                author="wrapper-bot",
                wake_eligible=False,
            ),
        )

    async def issue_reference(
        self, repository_full_name: str, issue_number: int
    ) -> IssueReference:
        return self.issue

    async def current_associated_pull_request(
        self, repository_full_name: str, issue_number: int
    ) -> PullRequestReference | None:
        return self.pull_request

    async def issue_comments(
        self,
        repository_full_name: str,
        issue_number: int,
        *,
        self_logins=frozenset(),
    ) -> tuple[IssueCommentSnapshot, ...]:
        return self.comments

    async def pull_request_state(
        self,
        reference: PullRequestReference,
        *,
        self_logins=frozenset(),
    ) -> PullRequestCanonicalState:
        return self.pr_state

    async def repository_permission(
        self, repository_full_name: str, actor_login: str
    ) -> RepositoryPermission:
        return RepositoryPermission(permission="write", role_name="write")


class ReconciliationTests(unittest.TestCase):
    def test_reconciliation_repairs_refs_without_persisting_comment_bodies(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeAuthority()
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                reconciler = GitHubReconciler(
                    authority,
                    store,
                    owner,
                    self_logins=frozenset({"wrapper-bot", "agent-bot"}),
                    clock=lambda: 2_000.0,
                )

                first = await reconciler.reconcile_binding(binding())
                self.assertTrue(first.issue_event_created)
                self.assertEqual(first.comment_events_created, 3)
                self.assertEqual(first.permissions_resolved, 1)
                self.assertEqual(first.current_pr_node_id, "PR_candidate")
                self.assertEqual(first.pull_request_events_created, 1)
                self.assertEqual(first.pull_request_objects_seen, 1)
                route = await store.current_pr_route("binding-1")
                assert route is not None
                self.assertEqual(route.surface_node_id, "PR_candidate")
                self.assertEqual(
                    await store.binding_for_surface("PR_candidate"), binding()
                )

                pending = await store.pending_events(owner, "binding-1")
                self.assertEqual(len(pending), 4)
                urgent = next(
                    event for event in pending if event.object_node_id == "IC_urgent"
                )
                self.assertTrue(urgent.urgent)
                self.assertEqual(urgent.permission_role, "write")
                own = await store.latest_event_for_object(
                    owner, "binding-1", "IC_self"
                )
                assert own is not None
                self.assertFalse(own.wake_eligible)
                self.assertEqual(own.state, EventState.SUPERSEDED)
                self.assertNotIn("ordinary discussion", repr(pending))

                unchanged = await reconciler.reconcile_binding(binding())
                self.assertFalse(unchanged.issue_event_created)
                self.assertEqual(unchanged.comment_events_created, 0)
                self.assertEqual(unchanged.permissions_resolved, 0)
                self.assertEqual(unchanged.pull_request_events_created, 0)

                authority.comments = (
                    comment(
                        "IC_urgent",
                        "@agent",
                        version="2026-08-10T12:02:00Z",
                        minimized=True,
                    ),
                    authority.comments[2],
                )
                changed = await reconciler.reconcile_binding(binding())
                self.assertEqual(changed.comment_events_created, 2)
                deleted = await store.latest_event_for_object(
                    owner, "binding-1", "IC_ordinary"
                )
                minimized = await store.latest_event_for_object(
                    owner, "binding-1", "IC_urgent"
                )
                assert deleted is not None
                assert minimized is not None
                self.assertEqual(deleted.action, "deleted")
                self.assertEqual(minimized.action, "minimized")
                self.assertFalse(minimized.mention_detected)
                self.assertFalse(minimized.urgent)
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_reconciliation_fails_closed_when_issue_identity_drifts(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeAuthority()
            authority.issue = replace(authority.issue, issue_node_id="I_other")
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                reconciler = GitHubReconciler(authority, store, owner)
                with self.assertRaises(StateConflict):
                    await reconciler.reconcile_binding(binding())
                self.assertEqual(
                    await store.pending_events(owner, "binding-1"), ()
                )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_current_pr_canonical_lifecycle_is_projected_to_exact_surface(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeAuthority()
            authority.comments = ()
            reference = authority.pull_request
            authority.pr_state = PullRequestCanonicalState(
                pull_request=pr_snapshot(reference),
                conversation_comments=(
                    pr_comment("IC_pr_1", "conversation one"),
                    pr_comment("IC_pr_2", "conversation two", minimized=True),
                ),
                reviews=(
                    review("PRR_1", "review one"),
                    review("PRR_2", "review two", state="DISMISSED"),
                ),
                review_comments=(
                    pr_comment("PRRC_1", "diff one"),
                    pr_comment("PRRC_2", "diff two", minimized=True),
                ),
                review_threads=(
                    thread("PRRT_1", resolved=False),
                    thread("PRRT_2", resolved=True),
                ),
            )
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                reconciler = GitHubReconciler(
                    authority, store, owner, clock=lambda: 3_000.0
                )

                initial = await reconciler.reconcile_binding(binding())
                self.assertEqual(initial.pull_request_events_created, 9)
                self.assertEqual(initial.pull_request_objects_seen, 9)
                initial_actions = {
                    event.object_node_id: event.action
                    for event in await store.latest_events_for_surface(
                        owner,
                        "binding-1",
                        event_name="pull_request_review_thread",
                        surface_node_id="PR_candidate",
                    )
                }
                self.assertEqual(
                    initial_actions,
                    {"PRRT_1": "unresolved", "PRRT_2": "resolved"},
                )

                authority.pr_state = PullRequestCanonicalState(
                    pull_request=pr_snapshot(
                        reference,
                        head="b" * 40,
                        version="2026-08-10T12:20:00Z",
                    ),
                    conversation_comments=(
                        pr_comment(
                            "IC_pr_1",
                            "conversation one edited",
                            version="2026-08-10T12:21:00Z",
                        ),
                    ),
                    reviews=(
                        review(
                            "PRR_1",
                            "review one edited",
                            version="2026-08-10T12:22:00Z",
                        ),
                        authority.pr_state.reviews[1],
                    ),
                    review_comments=(
                        pr_comment("PRRC_1", "diff one", minimized=True),
                    ),
                    review_threads=(
                        thread("PRRT_1", resolved=True),
                        thread("PRRT_2", resolved=False),
                    ),
                )
                changed = await reconciler.reconcile_binding(binding())
                self.assertEqual(changed.pull_request_events_created, 8)
                await self._assert_latest_actions(
                    store,
                    owner,
                    {
                        "PR_candidate": "synchronize",
                        "IC_pr_1": "edited",
                        "IC_pr_2": "deleted",
                        "PRR_1": "edited",
                        "PRR_2": "dismissed",
                        "PRRC_1": "minimized",
                        "PRRC_2": "deleted",
                        "PRRT_1": "resolved",
                        "PRRT_2": "unresolved",
                    },
                )

                authority.pr_state = replace(
                    authority.pr_state,
                    pull_request=pr_snapshot(
                        reference,
                        state="CLOSED",
                        head="b" * 40,
                        version="2026-08-10T12:30:00Z",
                    ),
                    conversation_comments=(
                        pr_comment(
                            "IC_pr_1",
                            "conversation one edited",
                            version="2026-08-10T12:21:00Z",
                            minimized=True,
                        ),
                    ),
                    reviews=(
                        review(
                            "PRR_1",
                            "review one edited",
                            version="2026-08-10T12:22:00Z",
                            state="DISMISSED",
                        ),
                        authority.pr_state.reviews[1],
                    ),
                    review_comments=(pr_comment("PRRC_1", "diff one"),),
                )
                terminal = await reconciler.reconcile_binding(binding())
                self.assertEqual(terminal.pull_request_events_created, 4)
                await self._assert_latest_actions(
                    store,
                    owner,
                    {
                        "PR_candidate": "closed",
                        "IC_pr_1": "minimized",
                        "PRR_1": "dismissed",
                        "PRRC_1": "unminimized",
                    },
                )

                authority.pull_request = replace(reference, state="OPEN")
                authority.pr_state = replace(
                    authority.pr_state,
                    pull_request=pr_snapshot(
                        reference,
                        state="OPEN",
                        head="b" * 40,
                        version="2026-08-10T12:40:00Z",
                    ),
                    conversation_comments=(
                        pr_comment(
                            "IC_pr_1",
                            "conversation one edited",
                            version="2026-08-10T12:21:00Z",
                        ),
                    ),
                )
                reopened = await reconciler.reconcile_binding(binding())
                self.assertEqual(reopened.pull_request_events_created, 2)
                await self._assert_latest_actions(
                    store,
                    owner,
                    {"PR_candidate": "reopened", "IC_pr_1": "unminimized"},
                )

                for expected, snapshot in (
                    (
                        "ready_for_review",
                        pr_snapshot(
                            reference,
                            draft=False,
                            head="b" * 40,
                            version="2026-08-10T12:41:00Z",
                        ),
                    ),
                    (
                        "converted_to_draft",
                        pr_snapshot(
                            reference,
                            draft=True,
                            head="b" * 40,
                            version="2026-08-10T12:42:00Z",
                        ),
                    ),
                    (
                        "edited",
                        pr_snapshot(
                            reference,
                            draft=True,
                            head="b" * 40,
                            version="2026-08-10T12:43:00Z",
                            body="edited candidate body",
                        ),
                    ),
                ):
                    authority.pr_state = replace(
                        authority.pr_state, pull_request=snapshot
                    )
                    report = await reconciler.reconcile_binding(binding())
                    self.assertEqual(report.pull_request_events_created, 1)
                    latest = await store.latest_event_for_object(
                        owner, "binding-1", "PR_candidate"
                    )
                    assert latest is not None
                    self.assertEqual(latest.action, expected)

                pending = await store.pending_events(owner, "binding-1", limit=1000)
                self.assertTrue(pending)
                self.assertTrue(
                    all(event.surface_node_id == "PR_candidate" for event in pending if event.surface_kind == "pull_request")
                )
                self.assertNotIn("conversation one", repr(pending))
                self.assertNotIn("review one", repr(pending))
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    async def _assert_latest_actions(
        self,
        store: TransportStore,
        owner,
        expected: dict[str, str],
    ) -> None:
        for node_id, action in expected.items():
            latest = await store.latest_event_for_object(
                owner, "binding-1", node_id
            )
            assert latest is not None
            self.assertEqual(latest.action, action)


if __name__ == "__main__":
    unittest.main()
