from __future__ import annotations

import asyncio
import hashlib
from pathlib import Path
import tempfile
import unittest

from github_agent_bridge.app_server import ServerMessage
from github_agent_bridge.github_api import RemoteComment
from github_agent_bridge.mirror_publisher import TurnMirrorPublisher
from github_agent_bridge.provider_adapter import ProviderTurn
from github_agent_bridge.store import Binding, EventEnvelope, TransportStore
from github_agent_bridge.turn_controller import BindingTurnController


class Clock:
    def __init__(self) -> None:
        self.now = 30.0

    def __call__(self) -> float:
        return self.now


class ProviderStream:
    thread_address = "thread-black-box"

    def __init__(self, messages: list[ServerMessage]) -> None:
        self._messages = messages

    async def start_turn(self, events) -> ProviderTurn:
        if len(events) != 1:
            raise AssertionError("black-box journey expected one settled event")
        return ProviderTurn("turn-black-box", "client-message-black-box")

    async def steer_turn(self, turn_id, events) -> None:
        raise AssertionError("black-box journey did not schedule a steer")

    async def next_message(self, *, timeout: float) -> ServerMessage:
        return self._messages.pop(0)


class GitHubCommentSurface:
    def __init__(self) -> None:
        self.body = ""
        self.create_count = 0
        self.update_count = 0

    async def create_issue_comment(self, repository, number, body) -> RemoteComment:
        self.create_count += 1
        self.body = body
        return self._remote()

    async def update_issue_comment(self, repository, comment_id, body) -> RemoteComment:
        if comment_id != 71:
            raise AssertionError("Braid edited a different logical comment")
        self.update_count += 1
        self.body = body
        return self._remote()

    async def get_issue_comment(self, repository, comment_id) -> RemoteComment:
        return self._remote()

    async def find_issue_comments_by_evidence(self, *args, **kwargs):
        return (self._remote(),) if self.body else ()

    def _remote(self) -> RemoteComment:
        return RemoteComment(
            database_id=71,
            node_id="IC_black_box",
            url="https://github.example/comment/71",
            author_login="braid-wrapper[bot]",
            created_at="2026-08-11T00:00:00Z",
            updated_at="2026-08-11T00:00:01Z",
            body_digest="sha256:"
            + hashlib.sha256(self.body.encode("utf-8")).hexdigest(),
        )


def server_message(method: str, params: dict[str, object]) -> ServerMessage:
    return ServerMessage(
        method=method,
        params={
            "threadId": "thread-black-box",
            "turnId": "turn-black-box",
            **params,
        },
    )


def completed(item: dict[str, object]) -> ServerMessage:
    return server_message("item/completed", {"item": item})


class TurnMirrorBlackBoxTests(unittest.TestCase):
    def test_one_event_becomes_one_human_readable_active_to_final_comment(self) -> None:
        async def journey(database: Path) -> None:
            clock = Clock()
            store = await TransportStore.open(database, clock=clock)
            github = GitHubCommentSurface()
            binding = Binding(
                binding_id="binding-black-box",
                repository_node_id="R_repository",
                repository_full_name="owner/repository",
                issue_node_id="I_issue",
                issue_number=24,
                issue_url="https://github.example/owner/repository/issues/24",
                thread_address="thread-black-box",
                agent_identity="coding-agent-bot",
                wrapper_identity="braid-wrapper[bot]",
                trusted_permission="triage",
                instruction_digest="sha256:instructions",
            )
            try:
                owner = await store.acquire_owner("owner-black-box", 1_000)
                await store.put_binding(owner, binding)
                event, _ = await store.ingest_event(
                    owner,
                    EventEnvelope(
                        event_key="github-delivery:black-box",
                        delivery_id="black-box",
                        binding_id=binding.binding_id,
                        event_name="issue_comment",
                        action="created",
                        object_node_id="IC_human",
                        surface_kind="issue",
                        surface_node_id=binding.issue_node_id,
                        object_version="version-1",
                        body_digest="sha256:human-message",
                        canonical_url="https://github.example/comment/human",
                        observed_at=0.0,
                        actor_login="human",
                    ),
                )
                await store.schedule_event(
                    owner,
                    event.event_id,
                    quiet_window_seconds=30,
                    received_at=0,
                )
                provider = ProviderStream(
                    [
                        completed(
                            {
                                "id": "assistant-activity",
                                "type": "agentMessage",
                                "phase": "commentary",
                                "text": "I will inspect the Issue first.",
                            }
                        ),
                        completed(
                            {
                                "id": "reasoning-summary",
                                "type": "reasoning",
                                "summary": ["The requested check is read-only."],
                                "content": ["RAW_COT_BLACK_BOX"],
                            }
                        ),
                        server_message(
                            "item/started",
                            {
                                "item": {
                                    "id": "command",
                                    "type": "commandExecution",
                                    "status": "inProgress",
                                    "command": "gh issue view 24 --repo owner/repository",
                                    "commandActions": [],
                                    "cwd": "/worktrees/issue-24",
                                    "aggregatedOutput": None,
                                }
                            },
                        ),
                        server_message(
                            "item/commandExecution/outputDelta",
                            {"itemId": "command", "delta": "state: OPEN"},
                        ),
                        completed(
                            {
                                "id": "command",
                                "type": "commandExecution",
                                "status": "completed",
                                "command": "gh issue view 24 --repo owner/repository",
                                "commandActions": [],
                                "cwd": "/worktrees/issue-24",
                                "aggregatedOutput": "state: OPEN",
                                "durationMs": 12,
                                "exitCode": 0,
                            }
                        ),
                        completed(
                            {
                                "id": "final",
                                "type": "agentMessage",
                                "phase": "final_answer",
                                "text": "The Issue is open; no repository files changed.",
                            }
                        ),
                        ServerMessage(
                            method="turn/completed",
                            params={
                                "threadId": "thread-black-box",
                                "turn": {
                                    "id": "turn-black-box",
                                    "status": "completed",
                                    "items": [],
                                },
                            },
                        ),
                    ]
                )
                publisher = TurnMirrorPublisher(github, store, owner)
                controller = BindingTurnController(
                    store,
                    owner,
                    provider,
                    publisher,
                    mirror_message_count_threshold=2,
                    clock=clock,
                    claim_factory=lambda: "claim-black-box",
                )

                result = await controller.run_one_ready_turn(binding, now=30)

                assert result is not None
                self.assertEqual(result.terminal_status, "completed")
                self.assertEqual(github.create_count, 1)
                self.assertGreaterEqual(github.update_count, 1)
                self.assertTrue(github.body.startswith("## Final response"))
                self.assertIn("The Issue is open", github.body)
                self.assertIn("**Reasoning summary**", github.body)
                self.assertIn("<details>", github.body)
                self.assertIn("gh issue view 24", github.body)
                self.assertIn("state: OPEN", github.body)
                for forbidden in (
                    "<!--",
                    "RAW_COT_BLACK_BOX",
                    "turn-black-box",
                    "thread-black-box",
                    "assistant-activity",
                    '"method"',
                ):
                    self.assertNotIn(forbidden, github.body)
            finally:
                await store.close()

        with tempfile.TemporaryDirectory() as directory:
            asyncio.run(journey(Path(directory) / "state.sqlite3"))


if __name__ == "__main__":
    unittest.main()
