from __future__ import annotations

import base64
import json
import re
import unittest

from github_agent_bridge.app_server import ServerMessage
from github_agent_bridge.mirror_render import (
    ACTIVE_VISIBLE_TEXT,
    render_mirror_chunks,
)
from github_agent_bridge.turn_projection import ProjectionOverflow, TurnProjection


THREAD_ID = "thread-1"
TURN_ID = "turn-1"


def message(method: str, params: dict[str, object]) -> ServerMessage:
    return ServerMessage(
        method=method,
        params={"threadId": THREAD_ID, "turnId": TURN_ID, **params},
    )


def completed_item(item: dict[str, object]) -> ServerMessage:
    return message("item/completed", {"item": item})


class TurnProjectionTests(unittest.TestCase):
    def test_projection_excludes_raw_cot_and_keeps_summary_tools_and_latest_final(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            message(
                "item/reasoning/textDelta",
                {"delta": "RAW_COT_MUST_NEVER_APPEAR", "itemId": "reasoning-1"},
            )
        )
        projection.consume(
            completed_item(
                {
                    "id": "reasoning-1",
                    "type": "reasoning",
                    "summary": ["Public reasoning summary."],
                    "content": ["RAW_COT_MUST_NEVER_APPEAR"],
                }
            )
        )
        projection.consume(
            completed_item(
                {
                    "id": "command-1",
                    "type": "commandExecution",
                    "status": "completed",
                    "command": "printf 'tool --> output'",
                    "commandActions": [],
                    "cwd": "/worktree",
                    "aggregatedOutput": "tool --> output",
                    "exitCode": 0,
                }
            )
        )
        for item_id, text in (("answer-1", "Earlier"), ("answer-2", "Final answer")):
            projection.consume(
                completed_item(
                    {
                        "id": item_id,
                        "type": "agentMessage",
                        "phase": "final_answer",
                        "text": text,
                    }
                )
            )
        projection.consume(
            ServerMessage(
                method="turn/completed",
                params={
                    "threadId": THREAD_ID,
                    "turn": {"id": TURN_ID, "status": "completed", "items": []},
                },
            )
        )

        snapshot = projection.snapshot()
        self.assertEqual(snapshot.final_answer, "Final answer")
        self.assertEqual(snapshot.raw_reasoning_items_excluded, 1)
        rendered = render_mirror_chunks(snapshot, revision=4)
        self.assertEqual(len(rendered), 1)
        self.assertTrue(rendered[0].body.startswith("Final answer\n\n<!--"))
        self.assertNotIn("RAW_COT_MUST_NEVER_APPEAR", rendered[0].body)
        self.assertNotIn("tool --> output", rendered[0].body)

        hidden = _decode_single_hidden_payload(rendered[0].body)
        encoded_hidden = json.dumps(hidden, ensure_ascii=False)
        self.assertIn("Public reasoning summary.", encoded_hidden)
        self.assertIn("tool --> output", encoded_hidden)
        self.assertNotIn("RAW_COT_MUST_NEVER_APPEAR", encoded_hidden)
        self.assertEqual(hidden["revision"], 4)

    def test_active_and_non_success_terminal_visible_text_are_distinct(self) -> None:
        active = TurnProjection(THREAD_ID, TURN_ID)
        active_body = render_mirror_chunks(active.snapshot(), revision=0)[0].body
        self.assertTrue(active_body.startswith(ACTIVE_VISIBLE_TEXT))

        interrupted = TurnProjection(THREAD_ID, TURN_ID)
        interrupted.consume(
            ServerMessage(
                method="turn/completed",
                params={
                    "threadId": THREAD_ID,
                    "turn": {
                        "id": TURN_ID,
                        "status": "interrupted",
                        "items": [],
                    },
                },
            )
        )
        interrupted_body = render_mirror_chunks(
            interrupted.snapshot(), revision=1
        )[0].body
        self.assertTrue(interrupted_body.startswith("Wrapper 状态："))
        self.assertIn("中断", interrupted_body)
        self.assertNotIn("完成。", interrupted_body)

    def test_large_projection_splits_on_utf8_and_safe_base64_boundaries(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            completed_item(
                {
                    "id": "command-1",
                    "type": "commandExecution",
                    "status": "completed",
                    "command": "generate",
                    "commandActions": [],
                    "cwd": "/worktree",
                    "aggregatedOutput": "-->" + "工具输出" * 500,
                    "exitCode": 0,
                }
            )
        )
        projection.consume(
            completed_item(
                {
                    "id": "answer-1",
                    "type": "agentMessage",
                    "phase": "final_answer",
                    "text": "最终回复" * 400,
                }
            )
        )
        projection.consume(
            ServerMessage(
                method="turn/completed",
                params={
                    "threadId": THREAD_ID,
                    "turn": {"id": TURN_ID, "status": "completed", "items": []},
                },
            )
        )

        chunks = render_mirror_chunks(
            projection.snapshot(), revision=9, max_comment_bytes=1_024
        )
        self.assertGreater(len(chunks), 1)
        self.assertEqual({chunk.count for chunk in chunks}, {len(chunks)})
        for index, chunk in enumerate(chunks):
            self.assertEqual(chunk.index, index)
            self.assertLessEqual(len(chunk.body.encode("utf-8")), 1_024)
            self.assertEqual(chunk.body.count("-->"), 1)

    def test_projection_overflow_fails_explicitly(self) -> None:
        projection = TurnProjection(
            THREAD_ID, TURN_ID, max_items=1, max_text_characters=20
        )
        projection.consume(message("turn/started", {}))
        with self.assertRaises(ProjectionOverflow):
            projection.consume(message("item/unknown/delta", {"delta": "x"}))


def _decode_single_hidden_payload(body: str) -> dict[str, object]:
    match = re.search(r"\n([A-Za-z0-9+/=]+)\n-->\Z", body)
    if match is None:
        raise AssertionError("rendered mirror omitted encoded payload")
    return json.loads(base64.b64decode(match.group(1)).decode("utf-8"))


if __name__ == "__main__":
    unittest.main()
