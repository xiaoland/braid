from __future__ import annotations

import asyncio
from dataclasses import replace
from pathlib import Path
import sqlite3
import tempfile
import unittest

from github_agent_bridge.store import (
    SCHEMA_VERSION,
    Binding,
    EventEnvelope,
    EventState,
    InvalidTransition,
    LeaseHeld,
    MirrorChunkIntent,
    OutboxIntent,
    OutboxState,
    SchemaVersionError,
    StaleLease,
    StateConflict,
    SurfaceKind,
    SurfaceRoute,
    TransportStore,
)


class FakeClock:
    def __init__(self, now: float = 1_000.0) -> None:
        self.now = now

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds


def binding() -> Binding:
    return Binding(
        binding_id="binding-1",
        repository_node_id="repository-node-1",
        repository_full_name="owner/repository",
        issue_node_id="issue-node-1",
        issue_number=17,
        issue_url="https://github.example/owner/repository/issues/17",
        thread_address="opaque-provider-thread-1",
        agent_identity="agent-app",
        wrapper_identity="wrapper-app",
        trusted_permission="triage",
        instruction_digest="sha256:instructions",
    )


def event() -> EventEnvelope:
    return EventEnvelope(
        event_key="github-delivery:delivery-1",
        delivery_id="delivery-1",
        binding_id="binding-1",
        event_name="issue_comment",
        action="created",
        object_node_id="comment-node-1",
        surface_kind="issue",
        surface_node_id="issue-node-1",
        object_version="2026-08-10T12:00:00Z",
        body_digest="sha256:comment-body",
        canonical_url="https://github.example/owner/repository/issues/17#comment-1",
        observed_at=1_001.0,
        actor_node_id="actor-node-1",
        actor_login="human",
        author_association="MEMBER",
        mention_detected=True,
        urgent=False,
    )


def pr_route(*, node_id: str = "pr-node-1", number: int = 18) -> SurfaceRoute:
    return SurfaceRoute(
        binding_id="binding-1",
        surface_kind=SurfaceKind.PULL_REQUEST,
        repository_node_id="repository-node-1",
        repository_full_name="owner/repository",
        surface_node_id=node_id,
        surface_number=number,
        canonical_url=f"https://github.example/owner/repository/pull/{number}",
        association_version="sha256:association-1",
    )


class TransportStoreTests(unittest.TestCase):
    def test_schema_migrates_once_and_rejects_unknown_future_version(self) -> None:
        async def scenario(path: Path) -> None:
            first = await TransportStore.open(path)
            self.assertEqual(await first.schema_version(), SCHEMA_VERSION)
            await first.close()

            reopened = await TransportStore.open(path)
            self.assertEqual(await reopened.schema_version(), SCHEMA_VERSION)
            await reopened.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

            future = Path(directory) / "future.sqlite3"
            connection = sqlite3.connect(future)
            connection.execute("PRAGMA user_version = 99")
            connection.close()
            with self.assertRaises(SchemaVersionError):
                asyncio.run(TransportStore.open(future))
            check = sqlite3.connect(future)
            self.assertEqual(check.execute("PRAGMA journal_mode").fetchone()[0], "delete")
            check.close()

            with self.assertRaisesRegex(ValueError, "absolute"):
                asyncio.run(TransportStore.open(Path("relative.sqlite3")))

    def test_lease_fences_second_owner_and_stale_generation(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock()
            first = await TransportStore.open(path, clock=clock)
            second = await TransportStore.open(path, clock=clock)
            try:
                first_token = await first.acquire_owner("owner-a", 10)
                with self.assertRaises(LeaseHeld):
                    await second.acquire_owner("owner-b", 10)

                clock.advance(11)
                second_token = await second.acquire_owner("owner-b", 10)
                with self.assertRaises(StaleLease):
                    await first.put_binding(first_token, binding())
                await second.put_binding(second_token, binding())
                self.assertEqual(
                    await second.binding_for_issue("issue-node-1"), binding()
                )
                self.assertEqual(await second.get_binding("binding-1"), binding())

                renewed = await second.renew_owner(second_token, 20)
                self.assertEqual(renewed.generation, second_token.generation)
                await second.release_owner(renewed)
                third = await first.acquire_owner("owner-c", 10)
                self.assertGreater(third.generation, second_token.generation)
            finally:
                await first.close()
                await second.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_event_ingestion_is_idempotent_and_transition_is_forward_only(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 60)
                await store.put_binding(token, binding())
                stored, created = await store.ingest_event(token, event())
                duplicate, duplicate_created = await store.ingest_event(token, event())

                self.assertTrue(created)
                self.assertFalse(duplicate_created)
                self.assertEqual(stored.event_id, duplicate.event_id)
                later_duplicate, later_duplicate_created = await store.ingest_event(
                    token, replace(event(), observed_at=9_999.0)
                )
                self.assertFalse(later_duplicate_created)
                self.assertEqual(later_duplicate.event_id, stored.event_id)
                self.assertEqual(later_duplicate.observed_at, event().observed_at)
                self.assertEqual(stored.state, EventState.PENDING)
                authorized = await store.resolve_event_permission(
                    token, stored.event_id, "write"
                )
                self.assertTrue(authorized.urgent)
                self.assertEqual(authorized.permission_role, "write")
                self.assertEqual(
                    await store.pending_events(token, "binding-1"), (authorized,)
                )
                self.assertEqual(
                    await store.latest_event_for_object(
                        token, "binding-1", "comment-node-1"
                    ),
                    authorized,
                )
                self.assertEqual(
                    await store.latest_events_for_surface(
                        token,
                        "binding-1",
                        event_name="issue_comment",
                        surface_node_id="issue-node-1",
                    ),
                    (authorized,),
                )

                delivered = await store.transition_event(
                    token, stored.event_id, EventState.DELIVERED
                )
                self.assertEqual(delivered.state, EventState.DELIVERED)
                self.assertEqual(await store.pending_events(token, "binding-1"), ())
                with self.assertRaises(InvalidTransition):
                    await store.transition_event(
                        token, stored.event_id, EventState.SUPERSEDED
                    )

                with self.assertRaises(StateConflict):
                    await store.ingest_event(
                        token,
                        replace(event(), body_digest="sha256:different"),
                    )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_late_older_object_version_is_audited_without_waking(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 60)
                await store.put_binding(token, binding())
                newer, _ = await store.ingest_event(
                    token,
                    replace(
                        event(),
                        object_version="2026-08-10T12:02:00Z",
                    ),
                )
                await store.schedule_event(
                    token,
                    newer.event_id,
                    quiet_window_seconds=30,
                    received_at=100,
                )
                older, _ = await store.ingest_event(
                    token,
                    replace(
                        event(),
                        event_key="github-delivery:late-old",
                        delivery_id="late-old",
                        object_version="2026-08-10T12:01:00Z",
                        body_digest="sha256:older",
                    ),
                )
                self.assertEqual(older.state, EventState.SUPERSEDED)
                unchanged = await store.schedule_event(
                    token,
                    older.event_id,
                    quiet_window_seconds=30,
                    received_at=200,
                )
                self.assertEqual(unchanged.generation, 1)
                pending = await store.pending_events(token, "binding-1")
                self.assertEqual(
                    [value.event_id for value in pending], [newer.event_id]
                )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_only_an_unmaterialized_thread_address_can_be_replaced(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 60)
                await store.put_binding(token, binding())
                await store.require_unmaterialized_thread_address(
                    token,
                    "binding-1",
                    expected_thread_address="opaque-provider-thread-1",
                )
                replaced = await store.replace_unmaterialized_thread_address(
                    token,
                    "binding-1",
                    expected_thread_address="opaque-provider-thread-1",
                    replacement_thread_address="opaque-provider-thread-2",
                )
                self.assertEqual(
                    replaced.thread_address, "opaque-provider-thread-2"
                )

                delivered, _ = await store.ingest_event(token, event())
                await store.transition_event(
                    token, delivered.event_id, EventState.DELIVERED
                )
                with self.assertRaises(StateConflict):
                    await store.require_unmaterialized_thread_address(
                        token,
                        "binding-1",
                        expected_thread_address="opaque-provider-thread-2",
                    )
                with self.assertRaises(StateConflict):
                    await store.replace_unmaterialized_thread_address(
                        token,
                        "binding-1",
                        expected_thread_address="opaque-provider-thread-2",
                        replacement_thread_address="opaque-provider-thread-3",
                    )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_current_pr_surface_is_an_opaque_replaceable_wake_alias(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 60)
                await store.put_binding(token, binding())
                first = await store.replace_current_pr_route(
                    token, "binding-1", pr_route()
                )
                self.assertEqual(first, pr_route())
                self.assertEqual(await store.current_pr_route("binding-1"), first)
                self.assertEqual(
                    await store.binding_for_surface("pr-node-1"), binding()
                )

                second_route = pr_route(node_id="pr-node-2", number=19)
                second = await store.replace_current_pr_route(
                    token, "binding-1", second_route
                )
                self.assertEqual(second, second_route)
                self.assertIsNone(await store.binding_for_surface("pr-node-1"))
                self.assertEqual(
                    await store.binding_for_surface("pr-node-2"), binding()
                )

                cleared = await store.replace_current_pr_route(
                    token, "binding-1", None
                )
                self.assertIsNone(cleared)
                self.assertIsNone(await store.current_pr_route("binding-1"))
                self.assertIsNone(await store.binding_for_surface("pr-node-2"))
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_quiet_and_urgent_scheduling_preserve_one_active_turn(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock(100.0)
            store = await TransportStore.open(path, clock=clock)
            try:
                token = await store.acquire_owner("owner-a", 1_000)
                await store.put_binding(token, binding())
                ordinary, _ = await store.ingest_event(
                    token, replace(event(), observed_at=100.0)
                )
                snapshot = await store.schedule_event(
                    token,
                    ordinary.event_id,
                    quiet_window_seconds=30,
                    received_at=100.0,
                )
                self.assertEqual(snapshot.quiet_deadline, 130.0)
                self.assertEqual(snapshot.transport_status, "pending")
                self.assertEqual(
                    await store.claim_ready_events(
                        token,
                        "binding-1",
                        claim_handle="claim-1",
                        now=129.9,
                    ),
                    (),
                )
                claimed = await store.claim_ready_events(
                    token,
                    "binding-1",
                    claim_handle="claim-1",
                    now=130.0,
                )
                self.assertEqual([value.event_id for value in claimed], [ordinary.event_id])
                active = await store.replace_active_turn_handle(
                    token,
                    "binding-1",
                    expected_handle="claim-1",
                    active_turn_handle="turn-1",
                )
                self.assertEqual(active.active_turn_handle, "turn-1")
                await store.mark_events_delivered(
                    token, (ordinary.event_id,)
                )

                ordinary_two, _ = await store.ingest_event(
                    token,
                    replace(
                        event(),
                        event_key="github-delivery:delivery-2",
                        delivery_id="delivery-2",
                        object_node_id="comment-node-2",
                        observed_at=131.0,
                    ),
                )
                await store.schedule_event(
                    token,
                    ordinary_two.event_id,
                    quiet_window_seconds=30,
                    received_at=131.0,
                )
                urgent, _ = await store.ingest_event(
                    token,
                    replace(
                        event(),
                        event_key="github-delivery:delivery-3",
                        delivery_id="delivery-3",
                        object_node_id="comment-node-3",
                        observed_at=132.0,
                        permission_role="write",
                        urgent=True,
                    ),
                )
                active_pending = await store.schedule_event(
                    token,
                    urgent.event_id,
                    quiet_window_seconds=30,
                    received_at=132.0,
                )
                self.assertEqual(active_pending.transport_status, "active-pending")
                self.assertEqual(active_pending.urgent_generation, 1)
                self.assertEqual(
                    await store.claim_ready_events(
                        token,
                        "binding-1",
                        claim_handle="parallel-not-allowed",
                        now=132.0,
                    ),
                    (),
                )

                pending = await store.finish_active_turn(
                    token, "binding-1", active_turn_handle="turn-1"
                )
                self.assertEqual(pending.transport_status, "pending")
                next_batch = await store.claim_ready_events(
                    token,
                    "binding-1",
                    claim_handle="claim-2",
                    now=132.0,
                )
                self.assertEqual(
                    {value.event_id for value in next_batch},
                    {ordinary_two.event_id, urgent.event_id},
                )
                await store.replace_active_turn_handle(
                    token,
                    "binding-1",
                    expected_handle="claim-2",
                    active_turn_handle="turn-2",
                )
                await store.mark_events_delivered(
                    token, tuple(value.event_id for value in next_batch)
                )
                idle = await store.finish_active_turn(
                    token, "binding-1", active_turn_handle="turn-2"
                )
                self.assertEqual(idle.transport_status, "idle")
                self.assertIsNone(idle.quiet_deadline)

                echo, _ = await store.ingest_event(
                    token,
                    replace(
                        event(),
                        event_key="github-delivery:delivery-4",
                        delivery_id="delivery-4",
                        object_node_id="comment-node-4",
                        wake_eligible=False,
                    ),
                )
                unchanged = await store.schedule_event(
                    token,
                    echo.event_id,
                    quiet_window_seconds=30,
                    received_at=140.0,
                )
                self.assertEqual(unchanged.generation, 3)
                self.assertEqual(unchanged.transport_status, "idle")
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_restart_restarts_full_quiet_but_preserves_urgent_wake(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 1_000)
                await store.put_binding(token, binding())
                ordinary, _ = await store.ingest_event(token, event())
                await store.schedule_event(
                    token,
                    ordinary.event_id,
                    quiet_window_seconds=30,
                    received_at=100.0,
                )
                restarted = await store.restart_pending_quiet_window(
                    token,
                    "binding-1",
                    observed_at=500.0,
                    quiet_window_seconds=30,
                )
                self.assertEqual(restarted.quiet_deadline, 530.0)
                self.assertEqual(
                    await store.claim_ready_events(
                        token,
                        "binding-1",
                        claim_handle="too-early",
                        now=529.9,
                    ),
                    (),
                )

                urgent, _ = await store.ingest_event(
                    token,
                    replace(
                        event(),
                        event_key="github-delivery:urgent",
                        delivery_id="urgent",
                        object_node_id="comment-node-urgent",
                        permission_role="write",
                        urgent=True,
                    ),
                )
                await store.schedule_event(
                    token,
                    urgent.event_id,
                    quiet_window_seconds=30,
                    received_at=501.0,
                )
                urgent_restart = await store.restart_pending_quiet_window(
                    token,
                    "binding-1",
                    observed_at=900.0,
                    quiet_window_seconds=30,
                )
                self.assertEqual(urgent_restart.quiet_deadline, 900.0)
                claimed = await store.claim_ready_events(
                    token,
                    "binding-1",
                    claim_handle="urgent-claim",
                    now=900.0,
                )
                self.assertEqual(len(claimed), 2)
                with self.assertRaises(StateConflict):
                    await store.restart_pending_quiet_window(
                        token,
                        "binding-1",
                        observed_at=901.0,
                        quiet_window_seconds=30,
                    )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_uncertain_outbox_requires_remote_reconciliation_before_retry(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 60)
                await store.put_binding(token, binding())
                intent = OutboxIntent(
                    operation_key="mirror:create:turn-1",
                    binding_id="binding-1",
                    operation_kind="comment-create",
                    target_node_id="issue-node-1",
                    intended_digest="sha256:mirror-body",
                )
                pending, created = await store.enqueue_outbox(token, intent)
                duplicate, duplicate_created = await store.enqueue_outbox(token, intent)
                self.assertTrue(created)
                self.assertFalse(duplicate_created)
                self.assertEqual(pending, duplicate)

                sending = await store.start_outbox_send(token, intent.operation_key)
                self.assertEqual(sending.state, OutboxState.SENDING)
                self.assertEqual(sending.attempts, 1)
                uncertain = await store.recover_sending_outbox(
                    token, intent.operation_key
                )
                self.assertEqual(uncertain.state, OutboxState.UNCERTAIN)
                with self.assertRaises(InvalidTransition):
                    await store.start_outbox_send(token, intent.operation_key)

                retryable = await store.reconcile_outbox_absent(
                    token, intent.operation_key
                )
                self.assertEqual(retryable.state, OutboxState.PENDING)
                sending_again = await store.start_outbox_send(
                    token, intent.operation_key
                )
                self.assertEqual(sending_again.attempts, 2)
                uncertain_again = await store.mark_outbox_uncertain(
                    token, intent.operation_key
                )
                self.assertEqual(uncertain_again.state, OutboxState.UNCERTAIN)

                with self.assertRaises(StateConflict):
                    await store.acknowledge_outbox(
                        token,
                        intent.operation_key,
                        remote_id="comment-1",
                        remote_digest="sha256:different",
                    )
                acked = await store.acknowledge_outbox(
                    token,
                    intent.operation_key,
                    remote_id="comment-1",
                    remote_digest=intent.intended_digest,
                )
                self.assertEqual(acked.state, OutboxState.ACKED)
                self.assertEqual(acked.remote_id, "comment-1")
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_mirror_revision_preserves_remote_chunk_identity(self) -> None:
        async def scenario(path: Path) -> None:
            store = await TransportStore.open(path)
            try:
                token = await store.acquire_owner("owner-a", 60)
                await store.put_binding(token, binding())
                first = await store.prepare_mirror_revision(
                    token,
                    turn_id="turn-1",
                    binding_id="binding-1",
                    target_node_id="issue-node-1",
                    terminal_state=None,
                    revision=0,
                    aggregate_digest="sha256:aggregate-0",
                    chunks=(
                        MirrorChunkIntent(0, "sha256:body-0", "marker-0"),
                        MirrorChunkIntent(1, "sha256:body-1", "marker-1"),
                    ),
                )
                self.assertEqual(len(first), 2)
                recorded = await store.record_mirror_chunk_remote(
                    token,
                    turn_id="turn-1",
                    chunk_index=0,
                    expected_body_digest="sha256:body-0",
                    remote_id="51",
                    remote_url="https://github.example/comment/51",
                    remote_digest="sha256:body-0",
                )
                self.assertEqual(recorded.remote_id, "51")

                second = await store.prepare_mirror_revision(
                    token,
                    turn_id="turn-1",
                    binding_id="binding-1",
                    target_node_id="issue-node-1",
                    terminal_state="completed",
                    revision=1,
                    aggregate_digest="sha256:aggregate-1",
                    chunks=(
                        MirrorChunkIntent(0, "sha256:final", "marker-0"),
                    ),
                )
                self.assertEqual(len(second), 1)
                self.assertEqual(second[0].remote_id, "51")
                self.assertEqual(second[0].remote_digest, "sha256:body-0")
                all_chunks = await store.mirror_chunks(
                    "turn-1", active_only=False
                )
                self.assertEqual(len(all_chunks), 2)
                self.assertFalse(all_chunks[1].active)

                with self.assertRaises(InvalidTransition):
                    await store.prepare_mirror_revision(
                        token,
                        turn_id="turn-1",
                        binding_id="binding-1",
                        target_node_id="issue-node-1",
                        terminal_state=None,
                        revision=0,
                        aggregate_digest="sha256:old",
                        chunks=(
                            MirrorChunkIntent(0, "sha256:old", "marker-0"),
                        ),
                    )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))


if __name__ == "__main__":
    unittest.main()
