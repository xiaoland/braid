from __future__ import annotations

import asyncio
import hashlib
from pathlib import Path
import tempfile
import unittest

from github_agent_bridge.github_api import GitHubApiError, RemoteComment
from github_agent_bridge.mirror_publisher import (
    MirrorConflict,
    MirrorTarget,
    TurnMirrorPublisher,
)
from github_agent_bridge.mirror_render import render_mirror_chunks
from github_agent_bridge.store import (
    Binding,
    MirrorChunkIntent,
    OutboxIntent,
    TransportStore,
)
from github_agent_bridge.turn_projection import TurnProjectionSnapshot


def binding() -> Binding:
    return Binding(
        binding_id="binding-1",
        repository_node_id="R_repository",
        repository_full_name="owner/repository",
        issue_node_id="I_issue",
        issue_number=17,
        issue_url="https://github.example/owner/repository/issues/17",
        thread_address="thread-1",
        agent_identity="agent-bot",
        wrapper_identity="wrapper-bot",
        trusted_permission="triage",
        instruction_digest="sha256:instructions",
    )


def snapshot(*, status: str | None = None, final: str | None = None) -> TurnProjectionSnapshot:
    return TurnProjectionSnapshot(
        thread_id="thread-1",
        turn_id="turn-1",
        messages=(),
        terminal_status=status,
        final_answer=final,
        raw_reasoning_items_excluded=0,
    )


class FakeCommentAuthority:
    def __init__(self) -> None:
        self.next_id = 51
        self.bodies: dict[int, str] = {}
        self.create_calls = 0
        self.update_calls = 0
        self.fail_create_after_remote = False
        self.recovery_match_mode = "unique"

    async def create_issue_comment(self, repository, number, body):
        self.create_calls += 1
        comment_id = self.next_id
        self.next_id += 1
        self.bodies[comment_id] = body
        remote = self._remote(comment_id)
        if self.fail_create_after_remote:
            self.fail_create_after_remote = False
            raise GitHubApiError("simulated response loss")
        return remote

    async def update_issue_comment(self, repository, comment_id, body):
        self.update_calls += 1
        self.bodies[comment_id] = body
        return self._remote(comment_id)

    async def get_issue_comment(self, repository, comment_id):
        return self._remote(comment_id)

    async def find_issue_comments_by_evidence(
        self,
        repository,
        number,
        *,
        author_login,
        body_digest,
        created_after,
        created_before,
    ):
        matches = tuple(
            self._remote(comment_id)
            for comment_id, body in self.bodies.items()
            if self._remote(comment_id).body_digest == body_digest
            and author_login == "wrapper-bot"
        )
        if self.recovery_match_mode == "none":
            return ()
        if self.recovery_match_mode == "multiple" and matches:
            return matches + (matches[0],)
        return matches

    def _remote(self, comment_id: int) -> RemoteComment:
        body = self.bodies[comment_id]
        return RemoteComment(
            database_id=comment_id,
            node_id=f"IC_{comment_id}",
            url=f"https://github.example/comment/{comment_id}",
            author_login="wrapper-bot",
            created_at="2026-08-10T12:00:00Z",
            updated_at="2026-08-10T12:00:00Z",
            body_digest="sha256:"
            + hashlib.sha256(body.encode("utf-8")).hexdigest(),
        )


class MirrorPublisherTests(unittest.TestCase):
    def test_active_comment_is_edited_to_final_without_creating_a_second_comment(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                publisher = TurnMirrorPublisher(authority, store, owner)
                target = MirrorTarget("I_issue", 17)

                active = await publisher.publish(
                    binding=binding(),
                    target=target,
                    snapshot=snapshot(),
                    revision=0,
                )
                self.assertEqual(authority.create_calls, 1)
                self.assertEqual(authority.update_calls, 0)
                self.assertEqual(active[0].remote_id, "51")
                self.assertTrue(authority.bodies[51].startswith("> ⏳"))
                self.assertNotIn("<!--", authority.bodies[51])

                final = await publisher.publish(
                    binding=binding(),
                    target=target,
                    snapshot=snapshot(status="completed", final="最终回复"),
                    revision=1,
                )
                self.assertEqual(authority.create_calls, 1)
                self.assertEqual(authority.update_calls, 1)
                self.assertEqual(final[0].remote_id, "51")
                self.assertTrue(authority.bodies[51].startswith("## Final response"))

                fyi = await publisher.publish_fyi(
                    binding=binding(),
                    turn_id="turn-1",
                    target=MirrorTarget("PR_candidate", 23),
                    canonical_comment_url=final[0].remote_url or "",
                )
                assert fyi is not None
                self.assertEqual(fyi.database_id, 52)
                self.assertIn("canonical 回复见", authority.bodies[52])
                self.assertIn(final[0].remote_url or "", authority.bodies[52])
                duplicate = await publisher.publish_fyi(
                    binding=binding(),
                    turn_id="turn-1",
                    target=MirrorTarget("PR_candidate", 23),
                    canonical_comment_url=final[0].remote_url or "",
                )
                self.assertIsNone(duplicate)
                self.assertEqual(authority.create_calls, 2)
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_uncertain_create_is_reconciled_by_canonical_evidence_without_duplicate(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            authority.fail_create_after_remote = True
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                publisher = TurnMirrorPublisher(authority, store, owner)
                arguments = {
                    "binding": binding(),
                    "target": MirrorTarget("I_issue", 17),
                    "snapshot": snapshot(),
                    "revision": 0,
                }
                with self.assertRaises(GitHubApiError):
                    await publisher.publish(**arguments)
                self.assertEqual(authority.create_calls, 1)
                recovered = await publisher.publish(**arguments)
                self.assertEqual(authority.create_calls, 1)
                self.assertEqual(recovered[0].remote_id, "51")
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_next_revision_recovers_prior_remote_create_before_editing(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            authority.fail_create_after_remote = True
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                publisher = TurnMirrorPublisher(authority, store, owner)
                target = MirrorTarget("I_issue", 17)
                with self.assertRaises(GitHubApiError):
                    await publisher.publish(
                        binding=binding(),
                        target=target,
                        snapshot=snapshot(),
                        revision=0,
                    )

                final = await publisher.publish(
                    binding=binding(),
                    target=target,
                    snapshot=snapshot(status="interrupted"),
                    revision=1,
                )
                self.assertEqual(authority.create_calls, 1)
                self.assertEqual(authority.update_calls, 1)
                self.assertEqual(final[0].remote_id, "51")
                self.assertIn("interrupted", authority.bodies[51])
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_uncertain_create_without_unique_evidence_fails_closed(self) -> None:
        async def scenario(path: Path, mode: str) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            authority.fail_create_after_remote = True
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                publisher = TurnMirrorPublisher(authority, store, owner)
                arguments = {
                    "binding": binding(),
                    "target": MirrorTarget("I_issue", 17),
                    "snapshot": snapshot(),
                    "revision": 0,
                }
                with self.assertRaises(GitHubApiError):
                    await publisher.publish(**arguments)
                authority.recovery_match_mode = mode
                with self.assertRaisesRegex(MirrorConflict, "no unique"):
                    await publisher.publish(**arguments)
                self.assertEqual(authority.create_calls, 1)
            finally:
                await store.close()

        for mode in ("none", "multiple"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                asyncio.run(scenario(Path(directory) / "state.sqlite3", mode))

    def test_crashed_sending_create_reconciles_remote_before_retry(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                current_snapshot = snapshot()
                rendered = render_mirror_chunks(current_snapshot, revision=0)
                chunk = rendered[0]
                aggregate_digest = "sha256:" + hashlib.sha256(
                    chunk.body_digest.encode("ascii")
                ).hexdigest()
                await store.prepare_mirror_revision(
                    owner,
                    turn_id="turn-1",
                    binding_id="binding-1",
                    target_node_id="I_issue",
                    terminal_state=None,
                    revision=0,
                    aggregate_digest=aggregate_digest,
                    chunks=(
                        MirrorChunkIntent(
                            chunk_index=0,
                            body_digest=chunk.body_digest,
                            ownership_marker=chunk.ownership_marker,
                        ),
                    ),
                )
                operation_key = (
                    "mirror:turn-1:chunk:0:revision:0:comment-create"
                )
                await store.enqueue_outbox(
                    owner,
                    OutboxIntent(
                        operation_key=operation_key,
                        binding_id="binding-1",
                        operation_kind="comment-create",
                        target_node_id="I_issue",
                        intended_digest=chunk.body_digest,
                    ),
                )
                await store.start_outbox_send(owner, operation_key)
                remote = await authority.create_issue_comment(
                    "owner/repository", 17, chunk.body
                )

                publisher = TurnMirrorPublisher(authority, store, owner)
                recovered = await publisher.publish(
                    binding=binding(),
                    target=MirrorTarget("I_issue", 17),
                    snapshot=current_snapshot,
                    revision=0,
                )
                self.assertEqual(authority.create_calls, 1)
                self.assertEqual(recovered[0].remote_id, str(remote.database_id))
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_crashed_sending_update_is_acked_when_remote_already_matches(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                publisher = TurnMirrorPublisher(authority, store, owner)
                target = MirrorTarget("I_issue", 17)
                await publisher.publish(
                    binding=binding(),
                    target=target,
                    snapshot=snapshot(),
                    revision=0,
                )

                final_snapshot = snapshot(status="completed", final="最终回复")
                rendered = render_mirror_chunks(final_snapshot, revision=1)
                chunk = rendered[0]
                aggregate_digest = "sha256:" + hashlib.sha256(
                    chunk.body_digest.encode("ascii")
                ).hexdigest()
                await store.prepare_mirror_revision(
                    owner,
                    turn_id="turn-1",
                    binding_id="binding-1",
                    target_node_id="I_issue",
                    terminal_state="completed",
                    revision=1,
                    aggregate_digest=aggregate_digest,
                    chunks=(
                        MirrorChunkIntent(
                            chunk_index=0,
                            body_digest=chunk.body_digest,
                            ownership_marker=chunk.ownership_marker,
                        ),
                    ),
                )
                operation_key = (
                    "mirror:turn-1:chunk:0:revision:1:comment-update"
                )
                await store.enqueue_outbox(
                    owner,
                    OutboxIntent(
                        operation_key=operation_key,
                        binding_id="binding-1",
                        operation_kind="comment-update",
                        target_node_id="51",
                        intended_digest=chunk.body_digest,
                    ),
                )
                await store.start_outbox_send(owner, operation_key)
                authority.bodies[51] = chunk.body

                recovered = await publisher.publish(
                    binding=binding(),
                    target=target,
                    snapshot=final_snapshot,
                    revision=1,
                )
                self.assertEqual(authority.update_calls, 0)
                self.assertEqual(recovered[0].remote_id, "51")
                outbox, created = await store.enqueue_outbox(
                    owner,
                    OutboxIntent(
                        operation_key=operation_key,
                        binding_id="binding-1",
                        operation_kind="comment-update",
                        target_node_id="51",
                        intended_digest=chunk.body_digest,
                    ),
                )
                self.assertFalse(created)
                self.assertEqual(outbox.state.value, "acked")
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_human_edit_causes_conflict_instead_of_silent_overwrite(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            authority = FakeCommentAuthority()
            try:
                owner = await store.acquire_owner("owner-a", 60)
                await store.put_binding(owner, binding())
                publisher = TurnMirrorPublisher(authority, store, owner)
                target = MirrorTarget("I_issue", 17)
                await publisher.publish(
                    binding=binding(),
                    target=target,
                    snapshot=snapshot(),
                    revision=0,
                )
                authority.bodies[51] = "Human replaced the projection"
                with self.assertRaises(MirrorConflict):
                    await publisher.publish(
                        binding=binding(),
                        target=target,
                        snapshot=snapshot(status="completed", final="final"),
                        revision=1,
                    )
                self.assertEqual(authority.update_calls, 0)
                self.assertEqual(authority.bodies[51], "Human replaced the projection")
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))


if __name__ == "__main__":
    unittest.main()
