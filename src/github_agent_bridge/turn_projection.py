"""Reduce one app-server turn into a bounded, Human-facing message history."""

from __future__ import annotations

from dataclasses import dataclass, field
import json
import re
from typing import Any

from github_agent_bridge.app_server import AppServerProtocolError, ServerMessage


RAW_REASONING_METHODS = frozenset(
    {"item/reasoning/textDelta", "item/reasoning/rawContentDelta"}
)
TERMINAL_STATUSES = frozenset(
    {"completed", "interrupted", "failed", "inProgress"}
)
ASSISTANT_PHASES = frozenset({"commentary", "final_answer"})
TOOL_ITEM_TYPES = frozenset(
    {
        "collabAgentToolCall",
        "commandExecution",
        "dynamicToolCall",
        "fileChange",
        "imageGeneration",
        "imageView",
        "mcpToolCall",
        "webSearch",
    }
)
TOOL_STATUSES = frozenset({"inProgress", "completed", "failed", "declined"})
STRUCTURED_SECRET_KEY_PARTS = frozenset(
    {
        "accesstoken",
        "apikey",
        "authorization",
        "bearertoken",
        "credential",
        "environment",
        "password",
        "refreshtoken",
        "secret",
    }
)
TRUNCATION_TEMPLATE = "\n… truncated by Braid ({omitted} UTF-8 bytes omitted) …\n"

JsonScalar = str | int | bool | None


class ProjectionOverflow(AppServerProtocolError):
    """A bounded turn projection exceeded its total in-memory limit."""


@dataclass(frozen=True, slots=True)
class ProjectedToolCall:
    kind: str
    label: str
    status: str
    call: str | None
    call_language: str
    result: str | None
    result_language: str
    facts: tuple[tuple[str, JsonScalar], ...] = ()
    call_truncated: bool = False
    result_truncated: bool = False


@dataclass(frozen=True, slots=True)
class ProjectedMessage:
    sequence: int
    item_id: str
    kind: str
    lifecycle: str
    content: str | None = None
    phase: str | None = None
    tool: ProjectedToolCall | None = None


@dataclass(frozen=True, slots=True)
class ProjectionChange:
    changed: bool = False
    completed_messages: int = 0
    terminal: bool = False


@dataclass(frozen=True, slots=True)
class TurnProjectionSnapshot:
    thread_id: str
    turn_id: str
    messages: tuple[ProjectedMessage, ...]
    terminal_status: str | None
    final_answer: str | None
    raw_reasoning_items_excluded: int


@dataclass(slots=True)
class _ToolState:
    kind: str
    label: str
    status: str
    call: str | None = None
    call_language: str = "text"
    result: str | None = None
    result_language: str = "text"
    facts: dict[str, JsonScalar] = field(default_factory=dict)
    call_truncated: bool = False
    result_truncated: bool = False

    def semantic_value(self) -> tuple[object, ...]:
        return (
            self.kind,
            self.label,
            self.status,
            self.call,
            self.call_language,
            self.result,
            self.result_language,
            tuple(sorted(self.facts.items())),
            self.call_truncated,
            self.result_truncated,
        )


@dataclass(slots=True)
class _MessageState:
    sequence: int
    item_id: str
    kind: str
    completed: bool = False
    content: str = ""
    phase: str | None = None
    summary_parts: list[str] = field(default_factory=list)
    tool: _ToolState | None = None

    def semantic_value(self) -> tuple[object, ...]:
        return (
            self.kind,
            self.content,
            self.phase,
            tuple(self.summary_parts),
            None if self.tool is None else self.tool.semantic_value(),
        )


class TurnProjection:
    """Own only the bounded messages required to render one live turn."""

    def __init__(
        self,
        thread_id: str,
        turn_id: str,
        *,
        max_messages: int = 512,
        max_projection_bytes: int = 256 * 1024,
        max_tool_call_bytes: int = 8 * 1024,
        max_tool_result_bytes: int = 16 * 1024,
    ) -> None:
        if not thread_id or not turn_id:
            raise ValueError("thread_id and turn_id must not be empty")
        if min(
            max_messages,
            max_projection_bytes,
            max_tool_call_bytes,
            max_tool_result_bytes,
        ) < 1:
            raise ValueError("projection bounds must be positive")
        self.thread_id = thread_id
        self.turn_id = turn_id
        self._max_messages = max_messages
        self._max_projection_bytes = max_projection_bytes
        self._max_tool_call_bytes = max_tool_call_bytes
        self._max_tool_result_bytes = max_tool_result_bytes
        self._states: dict[str, _MessageState] = {}
        self._next_sequence = 1
        self._terminal_status: str | None = None
        self._raw_reasoning_items_excluded = 0

    def consume(self, message: ServerMessage) -> ProjectionChange:
        """Reduce one matching protocol notification into projection state."""

        if message.params.get("threadId") != self.thread_id:
            return ProjectionChange()
        message_turn_id = message.params.get("turnId") or _nested_turn_id(
            message.params
        )
        if message_turn_id != self.turn_id:
            return ProjectionChange()
        if message.method in RAW_REASONING_METHODS:
            self._raw_reasoning_items_excluded += 1
            return ProjectionChange()
        if message.method == "turn/completed":
            return self._complete_turn(message.params)
        if message.method == "item/started":
            return self._start_item(_object(message.params.get("item"), "item"))
        if message.method == "item/completed":
            return self._complete_item(_object(message.params.get("item"), "item"))
        if message.method == "item/agentMessage/delta":
            return self._append_assistant_delta(message.params)
        if message.method == "item/reasoning/summaryPartAdded":
            return self._add_reasoning_part(message.params)
        if message.method == "item/reasoning/summaryTextDelta":
            return self._append_reasoning_delta(message.params)
        if message.method == "item/commandExecution/outputDelta":
            return self._append_tool_result_delta(
                message.params, expected_kind="commandExecution"
            )
        if message.method == "item/fileChange/outputDelta":
            return self._append_tool_result_delta(
                message.params, expected_kind="fileChange"
            )
        if message.method == "item/fileChange/patchUpdated":
            return self._replace_file_change_delta(message.params)
        if message.method == "item/mcpToolCall/progress":
            return self._append_mcp_progress(message.params)
        return ProjectionChange()

    def snapshot(self) -> TurnProjectionSnapshot:
        messages = tuple(
            projected
            for state in sorted(self._states.values(), key=lambda item: item.sequence)
            if (projected := _project_state(state)) is not None
        )
        final_answer = None
        if self._terminal_status == "completed":
            candidates = [
                item.content
                for item in messages
                if item.kind == "assistant_message"
                and item.lifecycle == "completed"
                and item.phase == "final_answer"
                and item.content
            ]
            if candidates:
                final_answer = candidates[-1]
        return TurnProjectionSnapshot(
            thread_id=self.thread_id,
            turn_id=self.turn_id,
            messages=messages,
            terminal_status=self._terminal_status,
            final_answer=final_answer,
            raw_reasoning_items_excluded=self._raw_reasoning_items_excluded,
        )

    def _start_item(self, item: dict[str, Any]) -> ProjectionChange:
        item_type = _required_string(item, "type", "item")
        item_id = _required_string(item, "id", "item")
        kind = _message_kind(item_type)
        if kind is None:
            return ProjectionChange()
        state, created = self._state(item_id, kind)
        before = state.semantic_value()
        self._replace_state_from_item(state, item_type, item, completed=False)
        self._check_bounds()
        changed = created or state.semantic_value() != before
        return ProjectionChange(changed=changed and _project_state(state) is not None)

    def _complete_item(self, item: dict[str, Any]) -> ProjectionChange:
        item_type = _required_string(item, "type", "item")
        item_id = _required_string(item, "id", "item")
        kind = _message_kind(item_type)
        if kind is None:
            return ProjectionChange()
        state, _ = self._state(item_id, kind)
        completed_before = state.completed
        prior = state.semantic_value()
        self._replace_state_from_item(state, item_type, item, completed=True)
        authoritative = state.semantic_value()
        if completed_before and authoritative != prior:
            raise AppServerProtocolError(
                "one item id produced conflicting completed projection snapshots"
            )
        state.completed = True
        self._check_bounds()
        changed = not completed_before or authoritative != prior
        counts = int(not completed_before and _project_state(state) is not None)
        return ProjectionChange(changed=changed, completed_messages=counts)

    def _replace_state_from_item(
        self,
        state: _MessageState,
        item_type: str,
        item: dict[str, Any],
        *,
        completed: bool,
    ) -> None:
        if state.kind == "assistant_message":
            text = item.get("text")
            if completed:
                state.content = _required_string(item, "text", "agentMessage", allow_empty=True)
            elif text is not None:
                state.content = _optional_string(text) or ""
            state.phase = _assistant_phase(item.get("phase"))
        elif state.kind == "reasoning_summary":
            state.summary_parts = _summary_parts(item.get("summary"))
        else:
            state.tool = self._tool_projection(item_type, item, completed=completed)

    def _append_assistant_delta(self, params: dict[str, Any]) -> ProjectionChange:
        item_id = _required_string(params, "itemId", "agentMessage delta")
        delta = _required_string(params, "delta", "agentMessage delta", allow_empty=True)
        if not delta:
            return ProjectionChange()
        state, _ = self._state(item_id, "assistant_message")
        if state.completed:
            raise AppServerProtocolError("assistant delta arrived after item completion")
        state.content += delta
        self._check_bounds()
        return ProjectionChange(changed=True)

    def _add_reasoning_part(self, params: dict[str, Any]) -> ProjectionChange:
        item_id = _required_string(params, "itemId", "reasoning summary part")
        index = _summary_index(params)
        state, _ = self._state(item_id, "reasoning_summary")
        if state.completed:
            raise AppServerProtocolError("reasoning part arrived after item completion")
        _ensure_summary_part(state, index)
        return ProjectionChange()

    def _append_reasoning_delta(self, params: dict[str, Any]) -> ProjectionChange:
        item_id = _required_string(params, "itemId", "reasoning summary delta")
        index = _summary_index(params)
        delta = _required_string(
            params, "delta", "reasoning summary delta", allow_empty=True
        )
        if not delta:
            return ProjectionChange()
        state, _ = self._state(item_id, "reasoning_summary")
        if state.completed:
            raise AppServerProtocolError("reasoning delta arrived after item completion")
        _ensure_summary_part(state, index)
        state.summary_parts[index] += delta
        self._check_bounds()
        return ProjectionChange(changed=True)

    def _append_tool_result_delta(
        self, params: dict[str, Any], *, expected_kind: str
    ) -> ProjectionChange:
        item_id = _required_string(params, "itemId", "tool result delta")
        delta = _required_string(params, "delta", "tool result delta", allow_empty=True)
        if not delta:
            return ProjectionChange()
        state, _ = self._state(item_id, "tool_call")
        if state.completed:
            raise AppServerProtocolError("tool result delta arrived after completion")
        if state.tool is None:
            state.tool = _empty_tool(expected_kind)
        if state.tool.kind != expected_kind:
            raise AppServerProtocolError("tool delta changed item kind")
        bounded = _bounded_text(
            (state.tool.result or "") + delta, self._max_tool_result_bytes
        )
        state.tool.result = bounded.text
        state.tool.result_truncated = bounded.truncated
        self._check_bounds()
        return ProjectionChange(changed=True)

    def _replace_file_change_delta(self, params: dict[str, Any]) -> ProjectionChange:
        item_id = _required_string(params, "itemId", "file change patch")
        changes = params.get("changes")
        if not isinstance(changes, list):
            raise AppServerProtocolError("file change patch.changes is not an array")
        state, _ = self._state(item_id, "tool_call")
        if state.completed:
            raise AppServerProtocolError("file change patch arrived after completion")
        state.tool = self._file_change_tool(
            {"changes": changes, "status": "inProgress"}
        )
        self._check_bounds()
        return ProjectionChange(changed=True)

    def _append_mcp_progress(self, params: dict[str, Any]) -> ProjectionChange:
        item_id = _required_string(params, "itemId", "MCP progress")
        progress = _required_string(params, "message", "MCP progress", allow_empty=True)
        if not progress:
            return ProjectionChange()
        state, _ = self._state(item_id, "tool_call")
        if state.completed:
            raise AppServerProtocolError("MCP progress arrived after completion")
        if state.tool is None:
            state.tool = _empty_tool("mcpToolCall")
        if state.tool.kind != "mcpToolCall":
            raise AppServerProtocolError("MCP progress changed item kind")
        bounded = _bounded_text(
            (state.tool.result or "") + progress, self._max_tool_result_bytes
        )
        state.tool.result = bounded.text
        state.tool.result_truncated = bounded.truncated
        self._check_bounds()
        return ProjectionChange(changed=True)

    def _complete_turn(self, params: dict[str, Any]) -> ProjectionChange:
        turn = _object(params.get("turn"), "turn/completed.turn")
        status = turn.get("status")
        if status not in TERMINAL_STATUSES:
            raise AppServerProtocolError("turn/completed returned an unknown status")
        if self._terminal_status is not None:
            if self._terminal_status != status:
                raise AppServerProtocolError("turn terminal status changed on replay")
            return ProjectionChange(terminal=True)
        changed = False
        completed_messages = 0
        items = turn.get("items", [])
        if not isinstance(items, list):
            raise AppServerProtocolError("turn/completed.turn.items is not an array")
        for item in items:
            if not isinstance(item, dict):
                raise AppServerProtocolError("turn completed item is not an object")
            result = self._complete_item(item)
            changed = changed or result.changed
            completed_messages += result.completed_messages
        self._terminal_status = status
        return ProjectionChange(
            changed=True, completed_messages=completed_messages, terminal=True
        )

    def _state(self, item_id: str, kind: str) -> tuple[_MessageState, bool]:
        state = self._states.get(item_id)
        if state is not None:
            if state.kind != kind:
                raise AppServerProtocolError("one item id changed projection kind")
            return state, False
        if len(self._states) >= self._max_messages:
            raise ProjectionOverflow("turn projection exceeded message count bound")
        state = _MessageState(self._next_sequence, item_id, kind)
        self._next_sequence += 1
        self._states[item_id] = state
        return state, True

    def _check_bounds(self) -> None:
        total = 0
        for state in self._states.values():
            total += _utf8_size(state.content)
            total += sum(_utf8_size(part) for part in state.summary_parts)
            if state.tool is not None:
                total += _utf8_size(state.tool.call or "")
                total += _utf8_size(state.tool.result or "")
        if total > self._max_projection_bytes:
            raise ProjectionOverflow("turn projection exceeded total byte bound")

    def _tool_projection(
        self, item_type: str, item: dict[str, Any], *, completed: bool
    ) -> _ToolState:
        if item_type == "commandExecution":
            return self._command_tool(item, completed=completed)
        if item_type == "fileChange":
            return self._file_change_tool(item)
        if item_type == "mcpToolCall":
            return self._mcp_tool(item, completed=completed)
        if item_type == "dynamicToolCall":
            return self._dynamic_tool(item, completed=completed)
        if item_type == "collabAgentToolCall":
            return self._collab_tool(item, completed=completed)
        if item_type == "webSearch":
            return self._web_search_tool(item, completed=completed)
        if item_type == "imageView":
            return self._image_view_tool(item, completed=completed)
        if item_type == "imageGeneration":
            return self._image_generation_tool(item, completed=completed)
        raise AppServerProtocolError("unsupported tool item type")

    def _command_tool(self, item: dict[str, Any], *, completed: bool) -> _ToolState:
        command = _required_string(item, "command", "commandExecution")
        cwd = _required_string(item, "cwd", "commandExecution")
        call = command + "\n\n# Working directory\n" + cwd
        bounded_call = _bounded_text(call, self._max_tool_call_bytes)
        output = _optional_string(item.get("aggregatedOutput"))
        bounded_result = _bounded_optional(output, self._max_tool_result_bytes)
        facts = _selected_scalars(
            item, {"durationMs": "duration_ms", "exitCode": "exit_code"}
        )
        return _ToolState(
            kind="commandExecution",
            label="Command",
            status=_tool_status(item, default=_lifecycle_status(completed)),
            call=bounded_call.text,
            call_language="shell",
            result=bounded_result.text,
            result_language="text",
            facts=facts,
            call_truncated=bounded_call.truncated,
            result_truncated=bounded_result.truncated,
        )

    def _file_change_tool(self, item: dict[str, Any]) -> _ToolState:
        changes = item.get("changes")
        if not isinstance(changes, list):
            raise AppServerProtocolError("fileChange.changes is not an array")
        call_lines: list[str] = []
        result_sections: list[str] = []
        for index, raw_change in enumerate(changes):
            change = _object(raw_change, f"fileChange.changes[{index}]")
            path = _required_string(change, "path", "file change")
            diff = _required_string(change, "diff", "file change", allow_empty=True)
            kind = _patch_kind(change.get("kind"))
            call_lines.append(f"- {kind}: {path}")
            result_sections.append(f"# {kind}: {path}\n{diff}")
        bounded_call = _bounded_text(
            "\n".join(call_lines) or "No file changes reported.",
            self._max_tool_call_bytes,
        )
        bounded_result = _bounded_optional(
            "\n\n".join(result_sections) or None,
            self._max_tool_result_bytes,
        )
        return _ToolState(
            kind="fileChange",
            label="File changes",
            status=_tool_status(item, default="inProgress"),
            call=bounded_call.text,
            call_language="text",
            result=bounded_result.text,
            result_language="diff",
            facts={"change_count": len(changes)},
            call_truncated=bounded_call.truncated,
            result_truncated=bounded_result.truncated,
        )

    def _mcp_tool(self, item: dict[str, Any], *, completed: bool) -> _ToolState:
        server = _required_string(item, "server", "mcpToolCall")
        tool = _required_string(item, "tool", "mcpToolCall")
        call_value = _sanitize_json_value(item.get("arguments"))
        bounded_call = _bounded_text(
            _json_text(call_value), self._max_tool_call_bytes
        )
        result_text = _mcp_result_text(item.get("result"), item.get("error"))
        bounded_result = _bounded_optional(result_text, self._max_tool_result_bytes)
        facts = _selected_scalars(
            item, {"durationMs": "duration_ms", "readOnlyHint": "read_only"}
        )
        return _ToolState(
            kind="mcpToolCall",
            label=f"{server} · {tool}",
            status=_tool_status(item, default=_lifecycle_status(completed)),
            call=bounded_call.text,
            call_language="json",
            result=bounded_result.text,
            result_language="text",
            facts=facts,
            call_truncated=bounded_call.truncated,
            result_truncated=bounded_result.truncated,
        )

    def _dynamic_tool(self, item: dict[str, Any], *, completed: bool) -> _ToolState:
        tool = _required_string(item, "tool", "dynamicToolCall")
        namespace = _optional_string(item.get("namespace"))
        bounded_call = _bounded_text(
            _json_text(_sanitize_json_value(item.get("arguments"))),
            self._max_tool_call_bytes,
        )
        result_value = _dynamic_content(item.get("contentItems"))
        bounded_result = _bounded_optional(
            None if result_value is None else _json_text(result_value),
            self._max_tool_result_bytes,
        )
        facts = _selected_scalars(
            item, {"durationMs": "duration_ms", "success": "success"}
        )
        return _ToolState(
            kind="dynamicToolCall",
            label=f"{namespace} · {tool}" if namespace else tool,
            status=_tool_status(item, default=_lifecycle_status(completed)),
            call=bounded_call.text,
            call_language="json",
            result=bounded_result.text,
            result_language="json",
            facts=facts,
            call_truncated=bounded_call.truncated,
            result_truncated=bounded_result.truncated,
        )

    def _collab_tool(self, item: dict[str, Any], *, completed: bool) -> _ToolState:
        tool = _required_string(item, "tool", "collabAgentToolCall")
        prompt = _optional_string(item.get("prompt"))
        call_value = {
            key: value
            for key, value in (
                ("model", _optional_string(item.get("model"))),
                ("prompt", prompt),
                ("reasoning_effort", _optional_string(item.get("reasoningEffort"))),
            )
            if value is not None
        }
        bounded_call = _bounded_text(
            _json_text(call_value), self._max_tool_call_bytes
        )
        states = item.get("agentsStates")
        if not isinstance(states, dict):
            raise AppServerProtocolError("collabAgentToolCall.agentsStates is not an object")
        anonymous_states = []
        for index, raw_state in enumerate(states.values(), start=1):
            state = _object(raw_state, "collab agent state")
            anonymous_states.append(
                {
                    "agent": index,
                    "status": _required_string(state, "status", "collab agent state"),
                    **(
                        {"message": message}
                        if (message := _optional_string(state.get("message")))
                        else {}
                    ),
                }
            )
        bounded_result = _bounded_optional(
            _json_text(anonymous_states) if anonymous_states else None,
            self._max_tool_result_bytes,
        )
        receivers = item.get("receiverThreadIds")
        receiver_count = len(receivers) if isinstance(receivers, list) else 0
        return _ToolState(
            kind="collabAgentToolCall",
            label=f"Agent collaboration · {tool}",
            status=_tool_status(item, default=_lifecycle_status(completed)),
            call=bounded_call.text,
            call_language="json",
            result=bounded_result.text,
            result_language="json",
            facts={"receiver_count": receiver_count},
            call_truncated=bounded_call.truncated,
            result_truncated=bounded_result.truncated,
        )

    def _web_search_tool(self, item: dict[str, Any], *, completed: bool) -> _ToolState:
        query = _required_string(item, "query", "webSearch", allow_empty=True)
        call_value: dict[str, Any] = {"query": query}
        action = item.get("action")
        if action is not None:
            call_value["action"] = _web_search_action(action)
        bounded_call = _bounded_text(
            _json_text(call_value), self._max_tool_call_bytes
        )
        result = None
        facts: dict[str, JsonScalar] = {}
        results = item.get("results")
        if isinstance(results, list):
            facts["result_count"] = len(results)
            result = (
                "Braid omitted web-search result objects because this pinned "
                "app-server schema defines their fields as opaque."
            )
        return _ToolState(
            kind="webSearch",
            label="Web search",
            status=_lifecycle_status(completed),
            call=bounded_call.text,
            call_language="json",
            result=result,
            result_language="text",
            facts=facts,
            call_truncated=bounded_call.truncated,
        )

    def _image_view_tool(self, item: dict[str, Any], *, completed: bool) -> _ToolState:
        bounded_call = _bounded_text(
            _required_string(item, "path", "imageView"),
            self._max_tool_call_bytes,
        )
        return _ToolState(
            kind="imageView",
            label="View image",
            status=_lifecycle_status(completed),
            call=bounded_call.text,
            call_language="text",
            result="Image content is not copied into the GitHub comment.",
            result_language="text",
            call_truncated=bounded_call.truncated,
        )

    def _image_generation_tool(
        self, item: dict[str, Any], *, completed: bool
    ) -> _ToolState:
        prompt = _optional_string(item.get("revisedPrompt"))
        bounded_call = _bounded_optional(prompt, self._max_tool_call_bytes)
        result_parts = [_required_string(item, "result", "imageGeneration", allow_empty=True)]
        if saved_path := _optional_string(item.get("savedPath")):
            result_parts.append(f"Saved path: {saved_path}")
        bounded_result = _bounded_text(
            "\n\n".join(part for part in result_parts if part),
            self._max_tool_result_bytes,
        )
        status = _required_string(item, "status", "imageGeneration")
        return _ToolState(
            kind="imageGeneration",
            label="Generate image",
            status=status if status else _lifecycle_status(completed),
            call=bounded_call.text,
            call_language="text",
            result=bounded_result.text,
            result_language="text",
            call_truncated=bounded_call.truncated,
            result_truncated=bounded_result.truncated,
        )


@dataclass(frozen=True, slots=True)
class _BoundedText:
    text: str | None
    truncated: bool


def _bounded_optional(value: str | None, limit: int) -> _BoundedText:
    if value is None:
        return _BoundedText(None, False)
    return _bounded_text(value, limit)


def _bounded_text(value: str, limit: int) -> _BoundedText:
    encoded = value.encode("utf-8")
    if len(encoded) <= limit:
        return _BoundedText(value, False)
    notice = TRUNCATION_TEMPLATE.format(omitted=max(0, len(encoded) - limit))
    notice_size = _utf8_size(notice)
    if notice_size >= limit:
        return _BoundedText(_utf8_prefix(notice, limit), True)
    available = limit - notice_size
    prefix_size = max(1, (available * 2) // 3)
    suffix_size = available - prefix_size
    prefix = _utf8_prefix(value, prefix_size)
    suffix = _utf8_suffix(value, suffix_size)
    actual_omitted = len(encoded) - _utf8_size(prefix) - _utf8_size(suffix)
    notice = TRUNCATION_TEMPLATE.format(omitted=actual_omitted)
    while _utf8_size(prefix + notice + suffix) > limit and prefix:
        prefix = prefix[:-1]
    return _BoundedText(prefix + notice + suffix, True)


def _message_kind(item_type: str) -> str | None:
    if item_type == "agentMessage":
        return "assistant_message"
    if item_type == "reasoning":
        return "reasoning_summary"
    if item_type in TOOL_ITEM_TYPES:
        return "tool_call"
    return None


def _project_state(state: _MessageState) -> ProjectedMessage | None:
    lifecycle = "completed" if state.completed else "inProgress"
    if state.kind == "assistant_message":
        if not state.content:
            return None
        return ProjectedMessage(
            sequence=state.sequence,
            item_id=state.item_id,
            kind=state.kind,
            lifecycle=lifecycle,
            content=state.content,
            phase=state.phase or "phase_unknown",
        )
    if state.kind == "reasoning_summary":
        content = "\n\n".join(part for part in state.summary_parts if part)
        if not content:
            return None
        return ProjectedMessage(
            sequence=state.sequence,
            item_id=state.item_id,
            kind=state.kind,
            lifecycle=lifecycle,
            content=content,
        )
    if state.tool is None:
        return None
    return ProjectedMessage(
        sequence=state.sequence,
        item_id=state.item_id,
        kind=state.kind,
        lifecycle=lifecycle,
        tool=ProjectedToolCall(
            kind=state.tool.kind,
            label=state.tool.label,
            status=state.tool.status,
            call=state.tool.call,
            call_language=state.tool.call_language,
            result=state.tool.result,
            result_language=state.tool.result_language,
            facts=tuple(sorted(state.tool.facts.items())),
            call_truncated=state.tool.call_truncated,
            result_truncated=state.tool.result_truncated,
        ),
    )


def _empty_tool(kind: str) -> _ToolState:
    labels = {
        "commandExecution": "Command",
        "fileChange": "File changes",
        "mcpToolCall": "MCP tool",
    }
    return _ToolState(kind=kind, label=labels[kind], status="inProgress")


def _mcp_result_text(result: Any, error: Any) -> str | None:
    sections: list[str] = []
    if result is not None:
        value = _object(result, "mcpToolCall.result")
        content = value.get("content")
        if not isinstance(content, list):
            raise AppServerProtocolError("mcpToolCall.result.content is not an array")
        rendered = [_mcp_content_item(item) for item in content]
        sections.extend(item for item in rendered if item)
    if error is not None:
        value = _object(error, "mcpToolCall.error")
        sections.append("Error: " + _required_string(value, "message", "MCP error"))
    return "\n\n".join(sections) or None


def _mcp_content_item(value: Any) -> str | None:
    if isinstance(value, str):
        return value
    if not isinstance(value, dict):
        return None
    kind = value.get("type")
    if kind == "text" and isinstance(value.get("text"), str):
        return value["text"]
    if kind in {"image", "audio"}:
        mime = value.get("mimeType")
        return f"[{kind} content omitted{f': {mime}' if isinstance(mime, str) else ''}]"
    if kind == "resource_link":
        selected = {
            key: value[key]
            for key in ("name", "title", "uri", "description")
            if isinstance(value.get(key), str)
        }
        return _json_text(selected)
    if kind == "resource" and isinstance(value.get("resource"), dict):
        resource = value["resource"]
        selected = {
            key: resource[key]
            for key in ("uri", "mimeType", "text")
            if isinstance(resource.get(key), str)
        }
        if "blob" in resource:
            selected["blob"] = "[binary content omitted]"
        return _json_text(selected)
    return None


def _dynamic_content(value: Any) -> list[dict[str, str]] | None:
    if value is None:
        return None
    if not isinstance(value, list):
        raise AppServerProtocolError("dynamicToolCall.contentItems is not an array")
    result: list[dict[str, str]] = []
    for raw in value:
        item = _object(raw, "dynamic tool content")
        kind = _required_string(item, "type", "dynamic tool content")
        field = {"inputText": "text", "inputImage": "imageUrl", "inputAudio": "audioUrl"}.get(kind)
        if field is None:
            raise AppServerProtocolError("dynamic tool content type is unknown")
        result.append({"type": kind, field: _required_string(item, field, "dynamic tool content")})
    return result


def _web_search_action(value: Any) -> dict[str, Any]:
    action = _object(value, "webSearch.action")
    kind = _required_string(action, "type", "webSearch.action")
    fields = {
        "search": ("query", "queries"),
        "openPage": ("url",),
        "findInPage": ("url", "pattern"),
        "other": (),
    }.get(kind)
    if fields is None:
        raise AppServerProtocolError("webSearch.action type is unknown")
    return {"type": kind, **{key: action[key] for key in fields if action.get(key) is not None}}


def _sanitize_json_value(value: Any) -> Any:
    if isinstance(value, dict):
        result: dict[str, Any] = {}
        for raw_key, member in value.items():
            key = str(raw_key)
            if _is_structured_secret_key(key):
                result[key] = "[excluded by Braid]"
            else:
                result[key] = _sanitize_json_value(member)
        return result
    if isinstance(value, list):
        return [_sanitize_json_value(member) for member in value]
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    return f"[unsupported {type(value).__name__} omitted]"


def _json_text(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True)


def _is_structured_secret_key(value: str) -> bool:
    normalized = re.sub(r"[^a-z0-9]", "", value.casefold())
    return normalized in {"env", "token"} or any(
        part in normalized for part in STRUCTURED_SECRET_KEY_PARTS
    )


def _patch_kind(value: Any) -> str:
    kind = _object(value, "file change kind")
    result = _required_string(kind, "type", "file change kind")
    if result not in {"add", "delete", "update"}:
        raise AppServerProtocolError("file change kind is unknown")
    return result


def _tool_status(item: dict[str, Any], *, default: str) -> str:
    value = item.get("status")
    if value is None:
        return default
    if not isinstance(value, str) or value not in TOOL_STATUSES:
        raise AppServerProtocolError("tool item status is unknown")
    return value


def _lifecycle_status(completed: bool) -> str:
    return "completed" if completed else "inProgress"


def _selected_scalars(
    item: dict[str, Any], mapping: dict[str, str]
) -> dict[str, JsonScalar]:
    selected: dict[str, JsonScalar] = {}
    for source, target in mapping.items():
        value = item.get(source)
        if value is not None and isinstance(value, (str, int, bool)):
            selected[target] = value
    return selected


def _summary_parts(value: Any) -> list[str]:
    if value is None:
        return []
    if not isinstance(value, list) or any(not isinstance(part, str) for part in value):
        raise AppServerProtocolError("reasoning summary is not an array of strings")
    return list(value)


def _summary_index(params: dict[str, Any]) -> int:
    value = params.get("summaryIndex")
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise AppServerProtocolError("reasoning summary index is invalid")
    return value


def _ensure_summary_part(state: _MessageState, index: int) -> None:
    while len(state.summary_parts) <= index:
        state.summary_parts.append("")


def _assistant_phase(value: Any) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or value not in ASSISTANT_PHASES:
        raise AppServerProtocolError("agent message phase is unknown")
    return value


def _optional_string(value: Any) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str):
        raise AppServerProtocolError("optional protocol text is not a string")
    return value


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


def _required_string(
    value: dict[str, Any], key: str, owner: str, *, allow_empty: bool = False
) -> str:
    member = value.get(key)
    if not isinstance(member, str) or (not allow_empty and not member):
        raise AppServerProtocolError(f"{owner}.{key} is not a valid string")
    return member


def _utf8_size(value: str) -> int:
    return len(value.encode("utf-8"))


def _utf8_prefix(value: str, limit: int) -> str:
    if limit <= 0:
        return ""
    encoded = value.encode("utf-8")[:limit]
    return encoded.decode("utf-8", errors="ignore")


def _utf8_suffix(value: str, limit: int) -> str:
    if limit <= 0:
        return ""
    encoded = value.encode("utf-8")[-limit:]
    return encoded.decode("utf-8", errors="ignore")
