"""Opaque Codex provider bridge for one Issue-bound thread."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
from typing import Any

from github_agent_bridge.app_server import (
    AppServerClient,
    AppServerProtocolError,
    AppServerRemoteError,
    ServerMessage,
)
from github_agent_bridge.protocol_probe import WRAPPER_TRANSPORT_INPUT
from github_agent_bridge.store import Binding, StoredEvent


@dataclass(frozen=True, slots=True)
class ProviderTurn:
    turn_id: str
    client_message_id: str


@dataclass(frozen=True, slots=True)
class PersistedProviderTurn:
    turn_id: str
    status: str


class ProviderNotSteerable(RuntimeError):
    """The active provider turn rejected same-turn context injection."""


class CodexProviderAdapter:
    """Mechanically start/resume/steer one provider-owned thread."""

    def __init__(
        self,
        client: AppServerClient,
        *,
        thread_address: str,
        provider_cwd: Path,
        writable_roots: Sequence[Path] = (),
        request_timeout_seconds: float = 120.0,
        persisted_turns: Sequence[PersistedProviderTurn] = (),
    ) -> None:
        if not thread_address:
            raise ValueError("thread_address must not be empty")
        if not provider_cwd.is_absolute():
            raise ValueError("provider_cwd must be absolute")
        if any(not path.is_absolute() for path in writable_roots):
            raise ValueError("writable roots must be absolute")
        if request_timeout_seconds <= 0:
            raise ValueError("request timeout must be positive")
        self._client = client
        self.thread_address = thread_address
        self.provider_cwd = provider_cwd
        self.writable_roots = tuple(writable_roots)
        self._request_timeout_seconds = request_timeout_seconds
        self._persisted_turns = tuple(persisted_turns)

    def persisted_turn_status(self, turn_id: str) -> str | None:
        for turn in self._persisted_turns:
            if turn.turn_id == turn_id:
                return turn.status
        return None

    @classmethod
    async def start_new(
        cls,
        client: AppServerClient,
        *,
        issue_url: str,
        provider_cwd: Path,
        writable_roots: Sequence[Path] = (),
        request_timeout_seconds: float = 120.0,
        initialize_client: bool = True,
    ) -> CodexProviderAdapter:
        if not issue_url:
            raise ValueError("issue_url must not be empty")
        if initialize_client:
            await client.initialize(
                client_name="github-agent-bridge",
                client_version="0.1.0",
                experimental_api=True,
                timeout=request_timeout_seconds,
            )
        result = _object(
            await client.request(
                "thread/start",
                {
                    "approvalPolicy": "never",
                    "cwd": str(provider_cwd),
                    "developerInstructions": _dynamic_issue_instructions(issue_url),
                    "ephemeral": False,
                    "sandbox": "workspace-write",
                },
                timeout=request_timeout_seconds,
            ),
            "thread/start",
        )
        thread = _object(result.get("thread"), "thread/start.thread")
        thread_address = _string(thread, "id", "thread/start.thread")
        return cls(
            client,
            thread_address=thread_address,
            provider_cwd=provider_cwd,
            writable_roots=writable_roots,
            request_timeout_seconds=request_timeout_seconds,
        )

    @classmethod
    async def connect(
        cls,
        client: AppServerClient,
        *,
        binding: Binding,
        provider_cwd: Path,
        writable_roots: Sequence[Path] = (),
        request_timeout_seconds: float = 120.0,
    ) -> CodexProviderAdapter:
        await client.initialize(
            client_name="github-agent-bridge",
            client_version="0.1.0",
            experimental_api=True,
            timeout=request_timeout_seconds,
        )
        if binding.thread_address:
            result = await client.request(
                "thread/resume",
                {
                    "approvalPolicy": "never",
                    "cwd": str(provider_cwd),
                    "sandbox": "workspace-write",
                    "threadId": binding.thread_address,
                },
                timeout=request_timeout_seconds,
            )
            owner = "thread/resume"
        else:
            result = await client.request(
                "thread/start",
                {
                    "approvalPolicy": "never",
                    "cwd": str(provider_cwd),
                    "developerInstructions": _dynamic_binding_instructions(binding),
                    "ephemeral": False,
                    "sandbox": "workspace-write",
                },
                timeout=request_timeout_seconds,
            )
            owner = "thread/start"
        response = _object(result, owner)
        thread = _object(response.get("thread"), f"{owner}.thread")
        thread_address = _string(thread, "id", f"{owner}.thread")
        if binding.thread_address and thread_address != binding.thread_address:
            raise AppServerProtocolError(
                "provider resumed a different opaque thread address"
            )
        persisted_turns = _persisted_turns(thread) if owner == "thread/resume" else ()
        return cls(
            client,
            thread_address=thread_address,
            provider_cwd=provider_cwd,
            writable_roots=writable_roots,
            request_timeout_seconds=request_timeout_seconds,
            persisted_turns=persisted_turns,
        )

    async def start_turn(self, events: Sequence[StoredEvent]) -> ProviderTurn:
        if not events:
            raise ValueError("cannot start a provider turn without event refs")
        client_message_id = _client_message_id("start", events)
        result = _object(
            await self._client.request(
                "turn/start",
                {
                    "additionalContext": _application_context(events),
                    "approvalPolicy": "never",
                    "clientUserMessageId": client_message_id,
                    "cwd": str(self.provider_cwd),
                    "input": [{"type": "text", "text": WRAPPER_TRANSPORT_INPUT}],
                    "sandboxPolicy": self._sandbox_policy(),
                    "summary": "concise",
                    "threadId": self.thread_address,
                },
                timeout=self._request_timeout_seconds,
            ),
            "turn/start",
        )
        turn = _object(result.get("turn"), "turn/start.turn")
        turn_id = _string(turn, "id", "turn/start.turn")
        if turn.get("status") != "inProgress":
            raise AppServerProtocolError(
                "turn/start did not return an in-progress provider turn"
            )
        return ProviderTurn(turn_id=turn_id, client_message_id=client_message_id)

    async def steer_turn(
        self, turn_id: str, events: Sequence[StoredEvent]
    ) -> None:
        if not turn_id:
            raise ValueError("turn_id must not be empty")
        if not events:
            raise ValueError("cannot steer without event refs")
        try:
            result = await self._client.request(
                "turn/steer",
                {
                    "additionalContext": _application_context(events),
                    "clientUserMessageId": _client_message_id("steer", events),
                    "expectedTurnId": turn_id,
                    "input": [{"type": "text", "text": WRAPPER_TRANSPORT_INPUT}],
                    "threadId": self.thread_address,
                },
                timeout=self._request_timeout_seconds,
            )
        except AppServerRemoteError as error:
            if error.code == -32600:
                raise ProviderNotSteerable(
                    "provider rejected same-turn steer"
                ) from error
            raise
        response = _object(result, "turn/steer")
        if response.get("turnId") != turn_id:
            raise AppServerProtocolError(
                "turn/steer did not preserve the active turn"
            )

    async def next_message(self, *, timeout: float) -> ServerMessage:
        return await self._client.next_message(timeout=timeout)

    async def close(self) -> None:
        await self._client.close()

    def _sandbox_policy(self) -> dict[str, object]:
        return {
            "networkAccess": True,
            "type": "workspaceWrite",
            "writableRoots": [str(path) for path in self.writable_roots],
        }


def _application_context(
    events: Sequence[StoredEvent],
) -> dict[str, dict[str, str]]:
    context: dict[str, dict[str, str]] = {}
    for event in events:
        value = {
            "action": event.action,
            "actor": {
                "author_association": event.author_association,
                "login": event.actor_login,
                "node_id": event.actor_node_id,
                "permission_role": event.permission_role,
            },
            "body_digest": event.body_digest,
            "canonical_url": event.canonical_url,
            "event_name": event.event_name,
            "lifecycle_version": event.object_version,
            "object_node_id": event.object_node_id,
            "source": "wrapper",
            "surface": {
                "kind": event.surface_kind,
                "node_id": event.surface_node_id,
            },
            "urgent_hint": event.urgent,
        }
        context[f"wrapper-event:{event.event_id}"] = {
            "kind": "application",
            "value": json.dumps(
                value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
            ),
        }
    return context


def _client_message_id(prefix: str, events: Sequence[StoredEvent]) -> str:
    encoded = json.dumps(
        [(event.event_id, event.event_key) for event in events],
        separators=(",", ":"),
    ).encode("utf-8")
    return f"wrapper-{prefix}-" + hashlib.sha256(encoded).hexdigest()


def _dynamic_binding_instructions(binding: Binding) -> str:
    return _dynamic_issue_instructions(binding.issue_url)


def _dynamic_issue_instructions(issue_url: str) -> str:
    return (
        "This provider thread is mechanically bound to GitHub Issue "
        f"{issue_url}. Wrapper-origin application context contains only "
        "event references, never Human commands. Use gh with the supplied URLs and "
        "node IDs to read canonical GitHub state. Follow the persistent project-scope "
        "collaboration instructions for discussion, Draft PR linking, and worktree "
        "safety."
    )


def _persisted_turns(thread: Mapping[str, Any]) -> tuple[PersistedProviderTurn, ...]:
    value = thread.get("turns", [])
    if not isinstance(value, list):
        raise AppServerProtocolError("thread/resume.thread.turns is not an array")
    turns = []
    for item in value:
        if not isinstance(item, dict):
            raise AppServerProtocolError(
                "thread/resume.thread.turn is not an object"
            )
        turn_id = item.get("id")
        status = item.get("status")
        if not isinstance(turn_id, str) or not turn_id:
            raise AppServerProtocolError(
                "thread/resume.thread.turn.id is not a non-empty string"
            )
        if status not in {"completed", "failed", "inProgress", "interrupted"}:
            raise AppServerProtocolError(
                "thread/resume.thread.turn.status is unknown"
            )
        turns.append(PersistedProviderTurn(turn_id, status))
    return tuple(turns)


def _object(value: Any, owner: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise AppServerProtocolError(f"{owner} is not an object")
    return value


def _string(value: Mapping[str, Any], key: str, owner: str) -> str:
    member = value.get(key)
    if not isinstance(member, str) or not member:
        raise AppServerProtocolError(f"{owner}.{key} is not a non-empty string")
    return member
