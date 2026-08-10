"""Allowlisted, non-persistent projection of one live Agent turn."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from github_agent_bridge.app_server import AppServerProtocolError, ServerMessage


RAW_REASONING_METHODS = frozenset(
    {"item/reasoning/textDelta", "item/reasoning/rawContentDelta"}
)
TERMINAL_STATUSES = frozenset(
    {"completed", "interrupted", "failed", "inProgress"}
)


class ProjectionOverflow(AppServerProtocolError):
    """A turn exceeded the bounded in-memory publication projection."""


@dataclass(frozen=True, slots=True)
class ProjectedProtocolItem:
    sequence: int
    method: str
    payload: dict[str, Any]


@dataclass(frozen=True, slots=True)
class TurnProjectionSnapshot:
    thread_id: str
    turn_id: str
    items: tuple[ProjectedProtocolItem, ...]
    terminal_status: str | None
    final_answer: str | None
    raw_reasoning_items_excluded: int


class TurnProjection:
    """Consume protocol messages without persisting provider thread history."""

    def __init__(
        self,
        thread_id: str,
        turn_id: str,
        *,
        max_items: int = 10_000,
        max_text_characters: int = 8 * 1024 * 1024,
    ) -> None:
        if not thread_id or not turn_id:
            raise ValueError("thread_id and turn_id must not be empty")
        if max_items < 1 or max_text_characters < 1:
            raise ValueError("projection bounds must be positive")
        self.thread_id = thread_id
        self.turn_id = turn_id
        self._max_items = max_items
        self._max_text_characters = max_text_characters
        self._items: list[ProjectedProtocolItem] = []
        self._text_characters = 0
        self._final_answers: list[str] = []
        self._terminal_status: str | None = None
        self._raw_reasoning_items_excluded = 0

    def consume(self, message: ServerMessage) -> bool:
        """Consume a matching turn message and report whether projection changed."""

        if message.params.get("threadId") != self.thread_id:
            return False
        message_turn_id = message.params.get("turnId") or _nested_turn_id(
            message.params
        )
        if message_turn_id != self.turn_id:
            return False
        if message.method in RAW_REASONING_METHODS:
            self._raw_reasoning_items_excluded += 1
            return False
        if message.method == "turn/completed":
            turn = _object(message.params.get("turn"), "turn/completed.turn")
            status = turn.get("status")
            if status not in TERMINAL_STATUSES:
                raise AppServerProtocolError(
                    "turn/completed returned an unknown terminal status"
                )
            self._terminal_status = status
            return True

        projected = _project_message(message)
        if projected is None:
            return False
        self._append(message.method, projected)
        if (
            message.method == "item/completed"
            and projected.get("type") == "agentMessage"
            and projected.get("phase") == "final_answer"
        ):
            text = projected.get("text")
            if isinstance(text, str):
                self._final_answers.append(text)
        return True

    def snapshot(self) -> TurnProjectionSnapshot:
        return TurnProjectionSnapshot(
            thread_id=self.thread_id,
            turn_id=self.turn_id,
            items=tuple(self._items),
            terminal_status=self._terminal_status,
            final_answer=(self._final_answers[-1] if self._final_answers else None),
            raw_reasoning_items_excluded=self._raw_reasoning_items_excluded,
        )

    def _append(self, method: str, payload: dict[str, Any]) -> None:
        if len(self._items) >= self._max_items:
            raise ProjectionOverflow("turn projection exceeded item count bound")
        text_characters = _text_character_count(payload)
        if self._text_characters + text_characters > self._max_text_characters:
            raise ProjectionOverflow("turn projection exceeded text size bound")
        self._text_characters += text_characters
        self._items.append(
            ProjectedProtocolItem(
                sequence=len(self._items) + 1,
                method=method,
                payload=payload,
            )
        )


def _project_message(message: ServerMessage) -> dict[str, Any] | None:
    if message.method in {
        "item/agentMessage/delta",
        "item/reasoning/summaryTextDelta",
        "item/commandExecution/outputDelta",
        "item/fileChange/outputDelta",
        "item/mcpToolCall/progress",
    }:
        return _allow_fields(
            message.params,
            ("delta", "itemId", "message", "output", "status"),
        )
    if message.method in {"item/started", "item/completed"}:
        item = _object(message.params.get("item"), f"{message.method}.item")
        return _project_item(item)
    if message.method == "turn/started":
        return {"status": "inProgress", "type": "turn"}
    if message.method.startswith("item/"):
        # Unknown turn-scoped item notifications are acknowledged without
        # serializing arbitrary params that may contain raw reasoning/secrets.
        return {"type": "unknownProtocolItem"}
    return None


def _project_item(item: dict[str, Any]) -> dict[str, Any]:
    item_type = item.get("type")
    if not isinstance(item_type, str):
        raise AppServerProtocolError("thread item type is missing")
    common = _allow_fields(item, ("id", "status", "type"))
    if item_type == "agentMessage":
        return {
            **common,
            **_allow_fields(item, ("phase", "text")),
        }
    if item_type == "reasoning":
        # `content` is raw CoT. Only provider-labelled summary is publishable.
        return {
            **common,
            **_allow_fields(item, ("summary",)),
        }
    if item_type == "plan":
        return {**common, **_allow_fields(item, ("text",))}
    if item_type == "commandExecution":
        return {
            **common,
            **_allow_fields(
                item,
                (
                    "aggregatedOutput",
                    "command",
                    "commandActions",
                    "cwd",
                    "durationMs",
                    "exitCode",
                    "source",
                ),
            ),
        }
    if item_type == "fileChange":
        return {**common, **_allow_fields(item, ("changes",))}
    if item_type in {"mcpToolCall", "dynamicToolCall", "collabAgentToolCall"}:
        return {
            **common,
            **_allow_fields(
                item,
                (
                    "arguments",
                    "durationMs",
                    "error",
                    "namespace",
                    "receiverThreadIds",
                    "result",
                    "server",
                    "tool",
                ),
            ),
        }
    if item_type in {"webSearch", "imageView"}:
        return {**common, **_allow_fields(item, ("query", "path"))}
    return {**common, "projection": "unknown-type-fields-excluded"}


def _allow_fields(value: dict[str, Any], names: tuple[str, ...]) -> dict[str, Any]:
    return {name: value[name] for name in names if name in value}


def _nested_turn_id(params: dict[str, Any]) -> str | None:
    turn = params.get("turn")
    if not isinstance(turn, dict):
        return None
    value = turn.get("id")
    return value if isinstance(value, str) else None


def _object(value: Any, owner: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise AppServerProtocolError(f"{owner} is not an object")
    return value


def _text_character_count(value: Any) -> int:
    if isinstance(value, str):
        return len(value)
    if isinstance(value, dict):
        return sum(_text_character_count(item) for item in value.values())
    if isinstance(value, list):
        return sum(_text_character_count(item) for item in value)
    return 0
