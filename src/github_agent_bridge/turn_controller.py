"""One-binding scheduler, provider, and mirror orchestration."""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass
import time
import uuid
from typing import Protocol

from github_agent_bridge.github_api import GitHubApiError
from github_agent_bridge.mirror_publisher import (
    MirrorConflict,
    MirrorTarget,
    TurnMirrorPublisher,
)
from github_agent_bridge.provider_adapter import (
    ProviderNotSteerable,
    ProviderTurn,
)
from github_agent_bridge.store import (
    Binding,
    LeaseToken,
    StateConflict,
    StoredEvent,
    StoredMirrorChunk,
    TransportStore,
)
from github_agent_bridge.turn_projection import TurnProjection


class ProviderBoundary(Protocol):
    thread_address: str

    async def start_turn(self, events: Sequence[StoredEvent]) -> ProviderTurn: ...

    async def steer_turn(
        self, turn_id: str, events: Sequence[StoredEvent]
    ) -> None: ...

    async def next_message(self, *, timeout: float): ...


@dataclass(frozen=True, slots=True)
class TurnRunResult:
    turn_id: str
    terminal_status: str
    final_answer: str | None
    mirror_error: str | None
    participating_surface_node_ids: tuple[str, ...]


class UnroutableSurface(StateConflict):
    """A formerly associated PR surface no longer belongs to the binding."""


class BindingTurnController:
    """Drive at most one provider turn for a binding.

    Webhook handling remains independent: it can durably schedule new events
    while this coroutine waits for provider notifications.
    """

    def __init__(
        self,
        store: TransportStore,
        owner_token: LeaseToken,
        provider: ProviderBoundary,
        mirror: TurnMirrorPublisher,
        *,
        mirror_interval_seconds: float = 5.0,
        provider_poll_seconds: float = 0.25,
        clock: Callable[[], float] = time.time,
        claim_factory: Callable[[], str] = lambda: str(uuid.uuid4()),
    ) -> None:
        if mirror_interval_seconds <= 0 or provider_poll_seconds <= 0:
            raise ValueError("controller intervals must be positive")
        self._store = store
        self._owner_token = owner_token
        self._provider = provider
        self._mirror = mirror
        self._mirror_interval_seconds = mirror_interval_seconds
        self._provider_poll_seconds = provider_poll_seconds
        self._clock = clock
        self._claim_factory = claim_factory

    async def run_one_ready_turn(
        self, binding: Binding, *, now: float | None = None
    ) -> TurnRunResult | None:
        claim_handle = self._claim_factory()
        claim_time = self._clock() if now is None else now
        events = await self._store.claim_ready_events(
            self._owner_token,
            binding.binding_id,
            claim_handle=claim_handle,
            now=claim_time,
        )
        if not events:
            return None
        try:
            target = await self._target_for_events(binding, events)
        except UnroutableSurface:
            await self._store.mark_events_superseded(
                self._owner_token,
                tuple(event.event_id for event in events),
            )
            await self._store.finish_active_turn(
                self._owner_token,
                binding.binding_id,
                active_turn_handle=claim_handle,
            )
            return None
        try:
            provider_turn = await self._provider.start_turn(events)
        except Exception:
            await self._store.finish_active_turn(
                self._owner_token,
                binding.binding_id,
                active_turn_handle=claim_handle,
            )
            raise
        try:
            await self._store.activate_claimed_turn(
                self._owner_token,
                binding.binding_id,
                expected_claim_handle=claim_handle,
                active_turn_handle=provider_turn.turn_id,
                delivered_event_ids=tuple(event.event_id for event in events),
            )
        except Exception:
            # The provider turn now exists. Preserve its real handle so recovery
            # fails closed instead of releasing the claim and starting a
            # parallel turn against the same Issue/thread.
            await self._store.replace_active_turn_handle(
                self._owner_token,
                binding.binding_id,
                expected_handle=claim_handle,
                active_turn_handle=provider_turn.turn_id,
            )
            raise

        projection = TurnProjection(
            self._provider.thread_address, provider_turn.turn_id
        )
        revision = 0
        mirror_error, published_chunks = await self._publish_best_effort(
            binding, target, projection, revision
        )
        next_mirror_at = self._clock() + self._mirror_interval_seconds
        projection_dirty = False
        participating = {event.surface_node_id for event in events}
        steer_rejected = False
        terminal = False
        try:
            while not terminal:
                try:
                    message = await self._provider.next_message(
                        timeout=self._provider_poll_seconds
                    )
                except TimeoutError:
                    message = None
                changed = False
                if message is not None:
                    changed = projection.consume(message)
                    projection_dirty = projection_dirty or changed
                    terminal = projection.snapshot().terminal_status is not None

                ready = await self._store.ready_events_for_active_turn(
                    self._owner_token,
                    binding.binding_id,
                    active_turn_handle=provider_turn.turn_id,
                    now=self._clock(),
                )
                if ready:
                    participating.update(event.surface_node_id for event in ready)
                if ready and not steer_rejected and not terminal:
                    try:
                        await self._provider.steer_turn(
                            provider_turn.turn_id, ready
                        )
                    except ProviderNotSteerable:
                        steer_rejected = True
                    else:
                        await self._store.mark_events_delivered(
                            self._owner_token,
                            tuple(event.event_id for event in ready),
                        )

                current_time = self._clock()
                if projection_dirty and (
                    terminal or current_time >= next_mirror_at
                ):
                    revision += 1
                    error, published_chunks = await self._publish_best_effort(
                        binding, target, projection, revision
                    )
                    mirror_error = error or mirror_error
                    projection_dirty = error is not None
                    next_mirror_at = current_time + self._mirror_interval_seconds
        finally:
            # A provider transport exception deliberately leaves the active
            # handle intact: disconnect is not an Agent terminal result.
            if terminal:
                await self._store.finish_active_turn(
                    self._owner_token,
                    binding.binding_id,
                    active_turn_handle=provider_turn.turn_id,
                )

        snapshot = projection.snapshot()
        assert snapshot.terminal_status is not None
        if len(participating) > 1 and published_chunks:
            canonical_url = published_chunks[0].remote_url
            if canonical_url is not None:
                for surface_node_id in sorted(participating - {target.surface_node_id}):
                    fyi_target = await self._target_for_surface_node(
                        binding, surface_node_id
                    )
                    try:
                        await self._mirror.publish_fyi(
                            binding=binding,
                            turn_id=provider_turn.turn_id,
                            target=fyi_target,
                            canonical_comment_url=canonical_url,
                        )
                    except (GitHubApiError, MirrorConflict) as error:
                        mirror_error = type(error).__name__
        return TurnRunResult(
            turn_id=provider_turn.turn_id,
            terminal_status=snapshot.terminal_status,
            final_answer=snapshot.final_answer,
            mirror_error=mirror_error,
            participating_surface_node_ids=tuple(sorted(participating)),
        )

    async def _target_for_events(
        self, binding: Binding, events: tuple[StoredEvent, ...]
    ) -> MirrorTarget:
        # The first locally observed event freezes the fallback canonical
        # surface. An explicit provider publication target can replace this in
        # a later protocol slice without text interpretation.
        first = events[0]
        if first.surface_kind == "issue":
            if first.surface_node_id != binding.issue_node_id:
                raise StateConflict("Issue event does not match binding surface")
            return MirrorTarget(binding.issue_node_id, binding.issue_number)
        if first.surface_kind == "pull_request":
            route = await self._store.current_pr_route(binding.binding_id)
            if route is None or route.surface_node_id != first.surface_node_id:
                raise UnroutableSurface(
                    "PR event has no current native association"
                )
            return MirrorTarget(route.surface_node_id, route.surface_number)
        raise StateConflict("event surface kind is not routable")

    async def _publish_best_effort(
        self,
        binding: Binding,
        target: MirrorTarget,
        projection: TurnProjection,
        revision: int,
    ) -> tuple[str | None, tuple[StoredMirrorChunk, ...]]:
        try:
            chunks = await self._mirror.publish(
                binding=binding,
                target=target,
                snapshot=projection.snapshot(),
                revision=revision,
            )
        except (GitHubApiError, MirrorConflict) as error:
            return type(error).__name__, ()
        return None, chunks

    async def _target_for_surface_node(
        self, binding: Binding, surface_node_id: str
    ) -> MirrorTarget:
        if surface_node_id == binding.issue_node_id:
            return MirrorTarget(binding.issue_node_id, binding.issue_number)
        route = await self._store.current_pr_route(binding.binding_id)
        if route is None or route.surface_node_id != surface_node_id:
            raise StateConflict("participating surface is no longer routable")
        return MirrorTarget(route.surface_node_id, route.surface_number)
