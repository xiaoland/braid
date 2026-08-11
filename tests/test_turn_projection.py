from __future__ import annotations

import unittest

from github_agent_bridge.app_server import AppServerProtocolError, ServerMessage
from github_agent_bridge.mirror_render import (
    ACTIVE_VISIBLE_TEXT,
    MirrorBodyOverflow,
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


def started_item(item: dict[str, object]) -> ServerMessage:
    return message("item/started", {"item": item})


def completed_item(item: dict[str, object]) -> ServerMessage:
    return message("item/completed", {"item": item})


def terminal(status: str = "completed") -> ServerMessage:
    return ServerMessage(
        method="turn/completed",
        params={
            "threadId": THREAD_ID,
            "turn": {"id": TURN_ID, "status": status, "items": []},
        },
    )


def command_item(
    *, output: str = "title: Mirror one turn\nstate: OPEN", command: str | None = None
) -> dict[str, object]:
    return {
        "id": "command-1",
        "type": "commandExecution",
        "status": "completed",
        "command": command or "gh issue view 23 --repo xiaoland/svc",
        "commandActions": [],
        "cwd": "/worktrees/issue-23",
        "aggregatedOutput": output,
        "durationMs": 855,
        "exitCode": 0,
        "processId": "must-not-render",
        "unknownProtocolField": "must-not-render",
    }


class TurnProjectionTests(unittest.TestCase):
    def test_complete_event_stream_renders_one_human_readable_comment(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            completed_item(
                {
                    "id": "assistant-1",
                    "type": "agentMessage",
                    "phase": "commentary",
                    "text": "我先读取当前 Issue，并核对工作区边界。",
                }
            )
        )
        projection.consume(
            message(
                "item/reasoning/textDelta",
                {"itemId": "reasoning-1", "delta": "RAW_COT_MUST_NOT_APPEAR"},
            )
        )
        projection.consume(
            completed_item(
                {
                    "id": "reasoning-1",
                    "type": "reasoning",
                    "summary": ["已确认任务只需要只读检查。"],
                    "content": ["RAW_COT_MUST_NOT_APPEAR"],
                }
            )
        )
        projection.consume(completed_item(command_item()))
        projection.consume(
            completed_item(
                {
                    "id": "answer-1",
                    "type": "agentMessage",
                    "phase": "final_answer",
                    "text": "已完成只读核对，没有修改仓库。",
                }
            )
        )
        projection.consume(terminal())

        body = render_mirror_chunks(projection.snapshot(), revision=4)[0].body

        expected = """## Final response

已完成只读核对，没有修改仓库。

### Turn activity

**Agent**

我先读取当前 Issue，并核对工作区边界。

**Reasoning summary**

已确认任务只需要只读检查。

<details>
<summary>✅ <strong>Command</strong> completed — exit 0 · 855 ms</summary>

**Call**

```shell
gh issue view 23 --repo xiaoland/svc

# Working directory
/worktrees/issue-23
```

**Result**

```text
title: Mirror one turn
state: OPEN
```

</details>"""
        self.assertEqual(body, expected)
        for forbidden in (
            "<!--",
            "RAW_COT_MUST_NOT_APPEAR",
            "command-1",
            "turn-1",
            "thread-1",
            "processId",
            "must-not-render",
            "ownership_marker",
            '"messages"',
        ):
            self.assertNotIn(forbidden, body)

    def test_started_deltas_and_completion_coalesce_one_tool_message(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            started_item(
                {
                    **command_item(output=""),
                    "status": "inProgress",
                    "aggregatedOutput": None,
                }
            )
        )
        projection.consume(
            message(
                "item/commandExecution/outputDelta",
                {"itemId": "command-1", "delta": "provisional output"},
            )
        )
        active = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
        self.assertEqual(active.count("<details>"), 1)
        self.assertIn("provisional output", active)
        self.assertIn("in progress", active)

        completion = projection.consume(completed_item(command_item(output="authoritative")))
        self.assertEqual(completion.completed_messages, 1)
        final = render_mirror_chunks(projection.snapshot(), revision=2)[0].body
        self.assertEqual(final.count("<details>"), 1)
        self.assertIn("authoritative", final)
        self.assertNotIn("provisional output", final)

    def test_supported_tool_matrix_projects_schema_known_payload_and_result(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        items = [
            command_item(),
            {
                "id": "file-1",
                "type": "fileChange",
                "status": "completed",
                "changes": [
                    {
                        "path": "src/braid.py",
                        "kind": {"type": "update", "move_path": None},
                        "diff": "@@ -1 +1 @@\n-old\n+new",
                    }
                ],
            },
            {
                "id": "mcp-1",
                "type": "mcpToolCall",
                "server": "github",
                "tool": "get_issue",
                "status": "completed",
                "arguments": {
                    "issue": 23,
                    "access_token": "credential-must-not-render",
                },
                "result": {
                    "content": [{"type": "text", "text": "Issue 23 is open"}],
                    "structuredContent": {"private": "must-not-render"},
                    "_meta": {"debug": "must-not-render"},
                },
                "error": None,
                "durationMs": 20,
                "readOnlyHint": True,
            },
            {
                "id": "dynamic-1",
                "type": "dynamicToolCall",
                "namespace": "workspace",
                "tool": "inspect",
                "status": "completed",
                "arguments": {"path": "README.md"},
                "contentItems": [{"type": "inputText", "text": "README content"}],
                "durationMs": 4,
                "success": True,
            },
            {
                "id": "collab-1",
                "type": "collabAgentToolCall",
                "tool": "wait",
                "status": "completed",
                "senderThreadId": "sender-must-not-render",
                "receiverThreadIds": ["receiver-must-not-render"],
                "prompt": "Audit the renderer",
                "model": "gpt-test",
                "reasoningEffort": "high",
                "agentsStates": {
                    "receiver-must-not-render": {
                        "status": "completed",
                        "message": "No findings",
                    }
                },
            },
            {
                "id": "search-1",
                "type": "webSearch",
                "query": "GitHub comment limit",
                "action": {"type": "search", "queries": ["GitHub comment limit"]},
                "results": [{"opaqueSecretField": "must-not-render"}],
            },
            {"id": "view-1", "type": "imageView", "path": "/tmp/render.png"},
            {
                "id": "image-1",
                "type": "imageGeneration",
                "status": "completed",
                "revisedPrompt": "Draw a braid",
                "result": "Image generated",
                "savedPath": "/tmp/braid.png",
            },
        ]
        for item in items:
            projection.consume(completed_item(item))

        body = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
        self.assertEqual(body.count("<details>"), 8)
        for expected in (
            "src/braid.py",
            "+new",
            "github · get_issue",
            '"issue": 23',
            "Issue 23 is open",
            "workspace · inspect",
            "README content",
            "Audit the renderer",
            "No findings",
            "GitHub comment limit",
            "schema defines their fields as opaque",
            "/tmp/render.png",
            "Draw a braid",
            "Image generated",
        ):
            self.assertIn(expected, body)
        for forbidden in (
            "credential-must-not-render",
            "sender-must-not-render",
            "receiver-must-not-render",
            "opaqueSecretField",
            "structuredContent",
            '"private"',
            '"debug"',
        ):
            self.assertNotIn(forbidden, body)
        self.assertIn("[excluded by Braid]", body)

    def test_multi_part_reasoning_and_partial_assistant_remain_visible(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            message(
                "item/agentMessage/delta",
                {"itemId": "assistant-1", "delta": "Still checking"},
            )
        )
        projection.consume(
            message(
                "item/reasoning/summaryPartAdded",
                {"itemId": "reasoning-1", "summaryIndex": 0},
            )
        )
        projection.consume(
            message(
                "item/reasoning/summaryTextDelta",
                {"itemId": "reasoning-1", "summaryIndex": 0, "delta": "Part one"},
            )
        )
        projection.consume(
            message(
                "item/reasoning/summaryTextDelta",
                {"itemId": "reasoning-1", "summaryIndex": 1, "delta": "Part two"},
            )
        )
        body = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
        self.assertTrue(body.startswith(ACTIVE_VISIBLE_TEXT))
        self.assertIn("**Agent _(partial)_**", body)
        self.assertIn("**Reasoning summary _(partial)_**", body)
        self.assertIn("Part one\n\nPart two", body)

    def test_tool_call_and_result_are_bounded_with_visible_notice(self) -> None:
        recursive = "gh issue view --comments\n" + ("comment body\n" * 200)
        projection = TurnProjection(
            THREAD_ID,
            TURN_ID,
            max_tool_call_bytes=256,
            max_tool_result_bytes=320,
        )
        projection.consume(
            completed_item(command_item(command="x" * 1_000, output=recursive))
        )
        tool = projection.snapshot().messages[0].tool
        assert tool is not None
        self.assertLessEqual(len((tool.call or "").encode("utf-8")), 256)
        self.assertLessEqual(len((tool.result or "").encode("utf-8")), 320)
        self.assertTrue(tool.call_truncated)
        self.assertTrue(tool.result_truncated)
        body = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
        self.assertIn("truncated by Braid", body)
        self.assertIn("truncated</summary>", body)

    def test_fence_length_prevents_payload_from_escaping_details(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            completed_item(command_item(output="before\n```\n</details>\nafter"))
        )
        body = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
        self.assertIn("````text\nbefore\n```\n</details>\nafter\n````", body)
        self.assertEqual(body.count("<details>"), 1)

    def test_provider_html_comment_syntax_is_made_visible(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            completed_item(
                {
                    "id": "assistant-1",
                    "type": "agentMessage",
                    "phase": "commentary",
                    "text": "before <!-- hidden --> after",
                }
            )
        )
        body = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
        self.assertNotIn("<!--", body)
        self.assertIn("before &lt;!-- hidden --&gt; after", body)

    def test_single_comment_omits_oldest_activity_instead_of_sharding(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        for index in range(12):
            projection.consume(
                completed_item(
                    {
                        "id": f"reasoning-{index}",
                        "type": "reasoning",
                        "summary": [f"summary-{index}-" + "x" * 180],
                        "content": ["raw"],
                    }
                )
            )
        projection.consume(
            completed_item(
                {
                    "id": "answer-1",
                    "type": "agentMessage",
                    "phase": "final_answer",
                    "text": "Final response must remain",
                }
            )
        )
        projection.consume(terminal())
        chunks = render_mirror_chunks(
            projection.snapshot(), revision=9, max_comment_bytes=1_024
        )
        self.assertEqual(len(chunks), 1)
        self.assertIn("Final response must remain", chunks[0].body)
        self.assertIn("Braid omitted", chunks[0].body)
        self.assertLessEqual(len(chunks[0].body.encode("utf-8")), 1_024)

    def test_final_response_is_never_silently_truncated(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            completed_item(
                {
                    "id": "answer-1",
                    "type": "agentMessage",
                    "phase": "final_answer",
                    "text": "x" * 2_000,
                }
            )
        )
        projection.consume(terminal())
        with self.assertRaises(MirrorBodyOverflow):
            render_mirror_chunks(
                projection.snapshot(), revision=1, max_comment_bytes=1_024
            )

    def test_terminal_statuses_do_not_claim_task_outcome(self) -> None:
        for status, text in (
            ("interrupted", "does not determine the task outcome"),
            ("failed", "task outcome remains unknown"),
        ):
            with self.subTest(status=status):
                projection = TurnProjection(THREAD_ID, TURN_ID)
                projection.consume(terminal(status))
                body = render_mirror_chunks(projection.snapshot(), revision=1)[0].body
                self.assertIn(text, body)

    def test_phase_unknown_is_visible_activity_but_never_guessed_as_final(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        projection.consume(
            completed_item({"id": "legacy-1", "type": "agentMessage", "text": "Legacy"})
        )
        projection.consume(terminal())
        snapshot = projection.snapshot()
        self.assertIsNone(snapshot.final_answer)
        self.assertIn("Legacy", render_mirror_chunks(snapshot, revision=1)[0].body)

    def test_projection_overflow_fails_explicitly(self) -> None:
        projection = TurnProjection(
            THREAD_ID, TURN_ID, max_messages=1, max_projection_bytes=20
        )
        projection.consume(
            message(
                "item/agentMessage/delta",
                {"itemId": "assistant-1", "delta": "x" * 20},
            )
        )
        with self.assertRaises(ProjectionOverflow):
            projection.consume(
                message(
                    "item/agentMessage/delta",
                    {"itemId": "assistant-1", "delta": "y"},
                )
            )

    def test_conflicting_completed_snapshot_fails_closed(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        first = {
            "id": "answer-1",
            "type": "agentMessage",
            "phase": "commentary",
            "text": "first",
        }
        projection.consume(completed_item(first))
        with self.assertRaises(AppServerProtocolError):
            projection.consume(completed_item({**first, "text": "different"}))

    def test_unknown_tool_status_fails_closed(self) -> None:
        projection = TurnProjection(THREAD_ID, TURN_ID)
        with self.assertRaisesRegex(AppServerProtocolError, "status is unknown"):
            projection.consume(
                completed_item({**command_item(), "status": "SECRET_TOKEN"})
            )


if __name__ == "__main__":
    unittest.main()
