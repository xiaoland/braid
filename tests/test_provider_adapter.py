from __future__ import annotations

import asyncio
import json
from pathlib import Path
import unittest

from github_agent_bridge.app_server import AppServerRemoteError, ServerMessage
from github_agent_bridge.provider_adapter import (
    CodexProviderAdapter,
    ProviderNotSteerable,
)
from github_agent_bridge.store import EventState, StoredEvent


def stored_event() -> StoredEvent:
    return StoredEvent(
        event_id=7,
        event_key="github-delivery:delivery-7",
        delivery_id="delivery-7",
        binding_id="binding-1",
        event_name="issue_comment",
        action="created",
        object_node_id="IC_comment",
        surface_kind="issue",
        surface_node_id="I_issue",
        object_version="2026-08-10T12:00:00Z",
        body_digest="sha256:body-only",
        canonical_url="https://github.example/owner/repository/issues/17#comment-7",
        observed_at=2_000.0,
        actor_node_id="U_human",
        actor_login="human",
        author_association="MEMBER",
        permission_role="write",
        mention_detected=True,
        urgent=True,
        wake_eligible=True,
        scheduled_at=2_000.0,
        state=EventState.PENDING,
    )


class FakeClient:
    def __init__(self) -> None:
        self.requests: list[tuple[str, dict[str, object], float]] = []
        self.response: object = {
            "turn": {"id": "turn-1", "status": "inProgress"}
        }
        self.closed = False

    async def request(self, method, params, *, timeout):
        self.requests.append((method, params, timeout))
        if isinstance(self.response, BaseException):
            raise self.response
        return self.response

    async def next_message(self, *, timeout):
        return ServerMessage(method="turn/started", params={})

    async def close(self):
        self.closed = True


class ProviderAdapterTests(unittest.TestCase):
    def test_turn_context_contains_refs_not_human_body(self) -> None:
        async def scenario() -> None:
            client = FakeClient()
            adapter = CodexProviderAdapter(
                client,  # type: ignore[arg-type]
                thread_address="thread-1",
                provider_cwd=Path("/worktrees/issue-17"),
                writable_roots=(Path("/repository/.git/worktrees/issue-17"),),
                request_timeout_seconds=10,
            )
            turn = await adapter.start_turn((stored_event(),))
            self.assertEqual(turn.turn_id, "turn-1")
            method, params, timeout = client.requests[0]
            self.assertEqual(method, "turn/start")
            self.assertEqual(timeout, 10)
            self.assertEqual(params["threadId"], "thread-1")
            self.assertEqual(
                params["sandboxPolicy"],
                {
                    "networkAccess": True,
                    "type": "workspaceWrite",
                    "writableRoots": [
                        "/repository/.git/worktrees/issue-17"
                    ],
                },
            )
            self.assertIn("clientUserMessageId", params)
            transport_input = params["input"]
            self.assertEqual(len(transport_input), 1)
            self.assertIn("This is not a Human message or command", transport_input[0]["text"])
            self.assertIn("observe canonical GitHub state", transport_input[0]["text"])
            context = params["additionalContext"]
            assert isinstance(context, dict)
            entry = context["wrapper-event:7"]
            assert isinstance(entry, dict)
            decoded = json.loads(entry["value"])
            self.assertEqual(decoded["source"], "wrapper")
            self.assertEqual(decoded["canonical_url"], stored_event().canonical_url)
            self.assertNotIn("Human message body", json.dumps(params))
            self.assertNotIn("raw_body", json.dumps(params))

        asyncio.run(scenario())

    def test_resume_exposes_only_provider_reported_turn_status(self) -> None:
        class ResumeClient(FakeClient):
            async def initialize(self, **_arguments):
                return {}

        async def scenario() -> None:
            client = ResumeClient()
            client.response = {
                "thread": {
                    "id": "opaque-provider-thread-1",
                    "turns": [
                        {"id": "turn-old", "status": "completed"},
                        {"id": "turn-active", "status": "interrupted"},
                    ],
                }
            }
            from github_agent_bridge.store import Binding

            adapter = await CodexProviderAdapter.connect(
                client,  # type: ignore[arg-type]
                binding=Binding(
                    binding_id="binding-1",
                    repository_node_id="R_repository",
                    repository_full_name="owner/repository",
                    issue_node_id="I_issue",
                    issue_number=17,
                    issue_url="https://github.example/issues/17",
                    thread_address="opaque-provider-thread-1",
                    agent_identity="agent-bot",
                    wrapper_identity="wrapper-bot",
                    trusted_permission="triage",
                    instruction_digest="sha256:instructions",
                ),
                provider_cwd=Path("/worktrees/issue-17"),
            )
            self.assertEqual(
                adapter.persisted_turn_status("turn-active"), "interrupted"
            )
            self.assertIsNone(adapter.persisted_turn_status("turn-missing"))

        asyncio.run(scenario())

    def test_read_only_turn_preserves_network_but_has_no_writable_roots(self) -> None:
        async def scenario() -> None:
            client = FakeClient()
            adapter = CodexProviderAdapter(
                client,  # type: ignore[arg-type]
                thread_address="thread-1",
                provider_cwd=Path("/worktrees/issue-17"),
                sandbox="read-only",
            )

            await adapter.start_turn((stored_event(),))

            _, params, _ = client.requests[-1]
            self.assertEqual(
                params["sandboxPolicy"],
                {"networkAccess": True, "type": "readOnly"},
            )

        asyncio.run(scenario())

    def test_same_turn_steer_is_preconditioned_and_rejection_is_typed(self) -> None:
        async def scenario() -> None:
            client = FakeClient()
            client.response = {"turnId": "turn-1"}
            adapter = CodexProviderAdapter(
                client,  # type: ignore[arg-type]
                thread_address="thread-1",
                provider_cwd=Path("/worktrees/issue-17"),
            )
            await adapter.steer_turn("turn-1", (stored_event(),))
            _, params, _ = client.requests[-1]
            self.assertEqual(params["expectedTurnId"], "turn-1")

            client.response = AppServerRemoteError(
                code=-32600, message="active turn not steerable"
            )
            with self.assertRaises(ProviderNotSteerable):
                await adapter.steer_turn("turn-1", (stored_event(),))

        asyncio.run(scenario())


if __name__ == "__main__":
    unittest.main()
