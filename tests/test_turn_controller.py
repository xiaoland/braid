from __future__ import annotations

import asyncio
from dataclasses import replace
from pathlib import Path
import tempfile
import unittest

from github_agent_bridge.app_server import ServerMessage
from github_agent_bridge.provider_adapter import ProviderTurn
from github_agent_bridge.store import (
    Binding,
    EventEnvelope,
    EventState,
    InvalidTransition,
    TransportStore,
)
from github_agent_bridge.turn_controller import BindingTurnController


class FakeClock:
    def __init__(self, now: float = 30.0) -> None:
        self.now = now

    def __call__(self) -> float:
        return self.now


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


def envelope(*, sequence: int = 1, urgent: bool = False) -> EventEnvelope:
    return EventEnvelope(
        event_key=f"github-delivery:delivery-{sequence}",
        delivery_id=f"delivery-{sequence}",
        binding_id="binding-1",
        event_name="issue_comment",
        action="created",
        object_node_id=f"IC_{sequence}",
        surface_kind="issue",
        surface_node_id="I_issue",
        object_version=f"version-{sequence}",
        body_digest=f"sha256:body-{sequence}",
        canonical_url=f"https://github.example/comment/{sequence}",
        observed_at=float(sequence),
        actor_login="human",
        permission_role="write" if urgent else None,
        mention_detected=urgent,
        urgent=urgent,
    )


class FakeProvider:
    thread_address = "thread-1"

    def __init__(self, messages, *, on_first_message=None, on_start=None) -> None:
        self.messages = list(messages)
        self.on_first_message = on_first_message
        self.on_start = on_start
        self.started_with = ()
        self.steered_with = []
        self.read_count = 0

    async def start_turn(self, events):
        self.started_with = tuple(events)
        if self.on_start is not None:
            await self.on_start()
        return ProviderTurn("turn-1", "client-message-1")

    async def steer_turn(self, turn_id, events):
        self.steered_with.append(tuple(events))

    async def next_message(self, *, timeout):
        if self.read_count == 0 and self.on_first_message is not None:
            await self.on_first_message()
        self.read_count += 1
        return self.messages.pop(0)


class FakeMirror:
    def __init__(self) -> None:
        self.publications = []

    async def publish(self, **arguments):
        self.publications.append(arguments)
        return ()


class TimedProvider(FakeProvider):
    def __init__(self, clock: FakeClock, messages) -> None:
        super().__init__(())
        self._clock = clock
        self._timed_messages = list(messages)

    async def next_message(self, *, timeout):
        instant, message = self._timed_messages.pop(0)
        self._clock.now = instant
        return message


def final_item() -> ServerMessage:
    return ServerMessage(
        method="item/completed",
        params={
            "threadId": "thread-1",
            "turnId": "turn-1",
            "item": {
                "id": "answer-1",
                "type": "agentMessage",
                "phase": "final_answer",
                "text": "Final answer",
            },
        },
    )


def reasoning_summary_delta() -> ServerMessage:
    return ServerMessage(
        method="item/reasoning/summaryTextDelta",
        params={
            "threadId": "thread-1",
            "turnId": "turn-1",
            "itemId": "reasoning-1",
            "delta": "正在核对关联。",
        },
    )


def terminal(status: str = "completed") -> ServerMessage:
    return ServerMessage(
        method="turn/completed",
        params={
            "threadId": "thread-1",
            "turn": {"id": "turn-1", "status": status, "items": []},
        },
    )


class TurnControllerTests(unittest.TestCase):
    def test_ready_batch_runs_one_turn_and_edits_same_mirror_to_final(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock()
            store = await TransportStore.open(path, clock=clock)
            try:
                owner = await store.acquire_owner("owner-a", 1_000)
                await store.put_binding(owner, binding())
                event, _ = await store.ingest_event(owner, envelope())
                await store.schedule_event(
                    owner,
                    event.event_id,
                    quiet_window_seconds=30,
                    received_at=0,
                )
                provider = FakeProvider((final_item(), terminal()))
                mirror = FakeMirror()
                controller = BindingTurnController(
                    store,
                    owner,
                    provider,
                    mirror,  # type: ignore[arg-type]
                    clock=clock,
                    claim_factory=lambda: "claim-1",
                )
                result = await controller.run_one_ready_turn(binding(), now=30)
                assert result is not None
                self.assertEqual(result.final_answer, "Final answer")
                self.assertEqual(result.terminal_status, "completed")
                self.assertEqual(len(provider.started_with), 1)
                self.assertEqual(len(mirror.publications), 2)
                self.assertIsNone(
                    mirror.publications[0]["snapshot"].terminal_status
                )
                self.assertEqual(
                    mirror.publications[1]["snapshot"].final_answer,
                    "Final answer",
                )
                latest = await store.latest_event_for_object(
                    owner, "binding-1", "IC_1"
                )
                assert latest is not None
                self.assertEqual(latest.state, EventState.DELIVERED)
                scheduler = await store.scheduler_snapshot("binding-1")
                self.assertEqual(scheduler.transport_status, "idle")
                self.assertIsNone(scheduler.active_turn_handle)
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_urgent_event_during_active_turn_is_steered_without_parallel_turn(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock()
            store = await TransportStore.open(path, clock=clock)
            owner = await store.acquire_owner("owner-a", 1_000)
            await store.put_binding(owner, binding())
            initial, _ = await store.ingest_event(owner, envelope())
            await store.schedule_event(
                owner,
                initial.event_id,
                quiet_window_seconds=30,
                received_at=0,
            )

            async def add_urgent() -> None:
                clock.now = 31
                urgent, _ = await store.ingest_event(
                    owner, envelope(sequence=2, urgent=True)
                )
                await store.schedule_event(
                    owner,
                    urgent.event_id,
                    quiet_window_seconds=30,
                    received_at=31,
                )

            provider = FakeProvider(
                (
                    ServerMessage(
                        method="turn/started",
                        params={
                            "threadId": "thread-1",
                            "turn": {"id": "turn-1"},
                        },
                    ),
                    terminal("interrupted"),
                ),
                on_first_message=add_urgent,
            )
            mirror = FakeMirror()
            controller = BindingTurnController(
                store,
                owner,
                provider,
                mirror,  # type: ignore[arg-type]
                clock=clock,
                claim_factory=lambda: "claim-1",
            )
            try:
                result = await controller.run_one_ready_turn(binding(), now=30)
                assert result is not None
                self.assertEqual(result.terminal_status, "interrupted")
                self.assertEqual(len(provider.steered_with), 1)
                self.assertEqual(provider.steered_with[0][0].object_node_id, "IC_2")
                urgent_latest = await store.latest_event_for_object(
                    owner, "binding-1", "IC_2"
                )
                assert urgent_latest is not None
                self.assertEqual(urgent_latest.state, EventState.DELIVERED)
                self.assertEqual(
                    result.participating_surface_node_ids, ("I_issue",)
                )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_changed_projection_is_flushed_on_tick_without_another_item(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock()
            store = await TransportStore.open(path, clock=clock)
            try:
                owner = await store.acquire_owner("owner-a", 1_000)
                await store.put_binding(owner, binding())
                stored, _ = await store.ingest_event(owner, envelope())
                await store.schedule_event(
                    owner,
                    stored.event_id,
                    quiet_window_seconds=30,
                    received_at=0,
                )
                provider = TimedProvider(
                    clock,
                    (
                        (31.0, reasoning_summary_delta()),
                        (
                            35.0,
                            ServerMessage(
                                method="thread/status/changed",
                                params={"threadId": "thread-1"},
                            ),
                        ),
                        (36.0, terminal()),
                    ),
                )
                mirror = FakeMirror()
                controller = BindingTurnController(
                    store,
                    owner,
                    provider,
                    mirror,  # type: ignore[arg-type]
                    mirror_interval_seconds=5,
                    clock=clock,
                    claim_factory=lambda: "claim-1",
                )
                result = await controller.run_one_ready_turn(binding(), now=30)
                assert result is not None
                self.assertEqual(len(mirror.publications), 3)
                tick_items = mirror.publications[1]["snapshot"].items
                self.assertEqual(tick_items[0].payload["delta"], "正在核对关联。")
                self.assertIsNone(
                    mirror.publications[1]["snapshot"].terminal_status
                )
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_unroutable_claim_is_superseded_without_starting_provider(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock()
            store = await TransportStore.open(path, clock=clock)
            try:
                owner = await store.acquire_owner("owner-a", 1_000)
                await store.put_binding(owner, binding())
                unroutable, _ = await store.ingest_event(
                    owner,
                    replace(
                        envelope(),
                        surface_kind="pull_request",
                        surface_node_id="PR_missing",
                    ),
                )
                await store.schedule_event(
                    owner,
                    unroutable.event_id,
                    quiet_window_seconds=30,
                    received_at=0,
                )
                provider = FakeProvider(())
                controller = BindingTurnController(
                    store,
                    owner,
                    provider,
                    FakeMirror(),  # type: ignore[arg-type]
                    clock=clock,
                    claim_factory=lambda: "claim-1",
                )
                result = await controller.run_one_ready_turn(binding(), now=30)
                self.assertIsNone(result)
                scheduler = await store.scheduler_snapshot("binding-1")
                self.assertIsNone(scheduler.active_turn_handle)
                self.assertEqual(scheduler.transport_status, "idle")
                latest = await store.latest_event_for_object(
                    owner, "binding-1", "IC_1"
                )
                assert latest is not None
                self.assertEqual(latest.state, EventState.SUPERSEDED)
                self.assertEqual(provider.started_with, ())
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))

    def test_activation_failure_preserves_real_provider_turn_handle(self) -> None:
        async def scenario(path: Path) -> None:
            clock = FakeClock()
            store = await TransportStore.open(path, clock=clock)
            try:
                owner = await store.acquire_owner("owner-a", 1_000)
                await store.put_binding(owner, binding())
                stored, _ = await store.ingest_event(owner, envelope())
                await store.schedule_event(
                    owner,
                    stored.event_id,
                    quiet_window_seconds=30,
                    received_at=0,
                )

                async def race_delivery() -> None:
                    await store.mark_events_delivered(owner, (stored.event_id,))

                controller = BindingTurnController(
                    store,
                    owner,
                    FakeProvider((), on_start=race_delivery),
                    FakeMirror(),  # type: ignore[arg-type]
                    clock=clock,
                    claim_factory=lambda: "claim-1",
                )
                with self.assertRaises(InvalidTransition):
                    await controller.run_one_ready_turn(binding(), now=30)
                scheduler = await store.scheduler_snapshot("binding-1")
                self.assertEqual(scheduler.active_turn_handle, "turn-1")
                self.assertEqual(scheduler.transport_status, "active")
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(scenario(Path(directory) / "state.sqlite3"))


if __name__ == "__main__":
    unittest.main()
