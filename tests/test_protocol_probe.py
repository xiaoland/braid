from __future__ import annotations

import asyncio
import unittest
from pathlib import Path

from github_agent_bridge.app_server import ServerMessage, provider_environment
from github_agent_bridge.protocol_probe import (
    TurnObservation,
    inspect_protocol_identity,
    _wait_for_terminal,
    latest_final_answer,
)


class ProviderEnvironmentTests(unittest.TestCase):
    def test_projects_allowlisted_provider_state_without_wrapper_secrets(self) -> None:
        projected = provider_environment(
            {
                "HOME": "/Users/operator",
                "LC_ALL": "C.UTF-8",
                "LC_WRAPPER_SECRET": "must-not-cross",
                "PATH": "/usr/bin",
                "WRAPPER_PRIVATE_KEY": "must-not-cross",
                "WRAPPER_WEBHOOK_SECRET": "must-not-cross",
            }
        )

        self.assertEqual(
            projected,
            {
                "HOME": "/Users/operator",
                "LC_ALL": "C.UTF-8",
                "PATH": "/usr/bin",
            },
        )


class TerminalProjectionTests(unittest.TestCase):
    def observation(self, *final_answers: str) -> TurnObservation:
        return TurnObservation(
            turn_id="turn-1",
            status="completed",
            final_answers=final_answers,
            commentary_messages=(),
            item_types=("agentMessage",),
            notification_methods=("turn/completed",),
        )

    def test_last_final_answer_wins_after_same_turn_steer(self) -> None:
        observation = self.observation("before steer", "after steer")

        self.assertEqual(latest_final_answer(observation), "after steer")

    def test_completed_without_final_is_explicitly_empty(self) -> None:
        self.assertIsNone(latest_final_answer(self.observation()))

    def test_completed_item_order_survives_repeated_item_ids(self) -> None:
        class Messages:
            def __init__(self, messages: list[ServerMessage]) -> None:
                self.messages = iter(messages)

            async def next_message(self, *, timeout: float) -> ServerMessage:
                del timeout
                return next(self.messages)

        def final(item_id: str, text: str) -> ServerMessage:
            return ServerMessage(
                method="item/completed",
                params={
                    "item": {
                        "id": item_id,
                        "phase": "final_answer",
                        "text": text,
                        "type": "agentMessage",
                    },
                    "threadId": "thread-1",
                    "turnId": "turn-1",
                },
            )

        messages = Messages(
            [
                final("a", "first"),
                final("b", "second"),
                final("a", "third"),
                ServerMessage(
                    method="turn/completed",
                    params={
                        "threadId": "thread-1",
                        "turn": {
                            "id": "turn-1",
                            "items": [],
                            "status": "completed",
                        },
                    },
                ),
            ]
        )

        observation = asyncio.run(
            _wait_for_terminal(messages, "thread-1", "turn-1", 1.0)  # type: ignore[arg-type]
        )

        self.assertEqual(observation.final_answers, ("first", "second", "third"))
        self.assertEqual(latest_final_answer(observation), "third")


class ProtocolIdentityTests(unittest.TestCase):
    def test_identity_inspection_uses_exact_schema_bundle_digests(self) -> None:
        # This guard exercises the packaged executable only when its explicit
        # opt-in environment is available; the real probe remains the authority.
        codex = Path("/definitely/not/a/codex/binary")
        with self.assertRaises(FileNotFoundError):
            asyncio.run(inspect_protocol_identity(codex_executable=codex))


if __name__ == "__main__":
    unittest.main()
