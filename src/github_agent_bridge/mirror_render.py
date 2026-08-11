"""Render one bounded Braid turn projection as Human-readable Markdown."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
import html
import re

from github_agent_bridge.turn_projection import (
    ProjectedMessage,
    ProjectedToolCall,
    TurnProjectionSnapshot,
)


ACTIVE_VISIBLE_TEXT = "> ⏳ **Agent 正在处理**"
DEFAULT_MAX_COMMENT_BYTES = 60_000


class MirrorBodyOverflow(ValueError):
    """The final response cannot fit in one GitHub comment without data loss."""


@dataclass(frozen=True, slots=True)
class RenderedMirrorChunk:
    index: int
    count: int
    body: str
    body_digest: str
    ownership_marker: str


def render_mirror_chunks(
    snapshot: TurnProjectionSnapshot,
    *,
    revision: int,
    max_comment_bytes: int = DEFAULT_MAX_COMMENT_BYTES,
) -> tuple[RenderedMirrorChunk, ...]:
    """Return exactly one visible comment while preserving the final response."""

    if revision < 0:
        raise ValueError("revision must not be negative")
    if max_comment_bytes < 1_024:
        raise ValueError("max_comment_bytes is too small for a safe mirror")
    final_index = _final_message_index(snapshot)
    activity = [
        message
        for index, message in enumerate(snapshot.messages)
        if index != final_index
    ]
    omitted = 0
    while True:
        body = _body(snapshot, activity, omitted=omitted)
        if len(body.encode("utf-8")) <= max_comment_bytes:
            return (_chunk(body, snapshot.turn_id),)
        if not activity:
            raise MirrorBodyOverflow(
                "final response exceeds the single-comment mirror bound"
            )
        activity.pop(0)
        omitted += 1


def _body(
    snapshot: TurnProjectionSnapshot,
    activity: list[ProjectedMessage],
    *,
    omitted: int,
) -> str:
    sections = [_status_section(snapshot)]
    activity_sections = [_render_message(message) for message in activity]
    activity_sections = [section for section in activity_sections if section]
    if omitted or activity_sections:
        sections.append("### Turn activity")
        if omitted:
            sections.append(
                "> ℹ️ Braid omitted "
                f"{omitted} earlier activity message{'s' if omitted != 1 else ''} "
                "to stay within the GitHub comment limit."
            )
        sections.extend(activity_sections)
    return "\n\n".join(section.rstrip() for section in sections if section).rstrip()


def _status_section(snapshot: TurnProjectionSnapshot) -> str:
    status = snapshot.terminal_status
    if status is None or status == "inProgress":
        return ACTIVE_VISIBLE_TEXT
    if status == "completed" and snapshot.final_answer is not None:
        return "## Final response\n\n" + _expose_html_comments(
            snapshot.final_answer
        ).rstrip()
    if status == "completed":
        return "> ℹ️ **Agent turn completed without a publishable final response.**"
    if status == "interrupted":
        return (
            "> ⚠️ **Agent turn was interrupted before a final response.**  \n"
            "> This does not determine the task outcome."
        )
    if status == "failed":
        return (
            "> ❌ **Agent turn failed before a final response.**  \n"
            "> The task outcome remains unknown."
        )
    return (
        "> ⚠️ **Agent turn status is unknown.**  \n"
        "> Braid has not retried work or claimed completion."
    )


def _final_message_index(snapshot: TurnProjectionSnapshot) -> int | None:
    if snapshot.terminal_status != "completed" or snapshot.final_answer is None:
        return None
    candidate = None
    for index, message in enumerate(snapshot.messages):
        if (
            message.kind == "assistant_message"
            and message.lifecycle == "completed"
            and message.phase == "final_answer"
            and message.content == snapshot.final_answer
        ):
            candidate = index
    return candidate


def _render_message(message: ProjectedMessage) -> str:
    if message.kind == "assistant_message" and message.content:
        qualifier = " _(partial)_" if message.lifecycle != "completed" else ""
        return f"**Agent{qualifier}**\n\n{_expose_html_comments(message.content).rstrip()}"
    if message.kind == "reasoning_summary" and message.content:
        qualifier = " _(partial)_" if message.lifecycle != "completed" else ""
        return (
            f"**Reasoning summary{qualifier}**\n\n"
            f"{_expose_html_comments(message.content).rstrip()}"
        )
    if message.kind == "tool_call" and message.tool is not None:
        return _render_tool(message.tool)
    return ""


def _render_tool(tool: ProjectedToolCall) -> str:
    summary = _tool_summary(tool)
    sections = ["<details>", f"<summary>{summary}</summary>"]
    if tool.call is not None:
        sections.extend(
            ["", "**Call**", "", _fenced(tool.call, tool.call_language)]
        )
    if tool.result is not None:
        sections.extend(
            ["", "**Result**", "", _fenced(tool.result, tool.result_language)]
        )
    if tool.call is None and tool.result is None:
        sections.extend(["", "_No schema-approved call or result text was provided._"])
    sections.extend(["", "</details>"])
    return "\n".join(sections)


def _tool_summary(tool: ProjectedToolCall) -> str:
    icon = {
        "inProgress": "⏳",
        "completed": "✅",
        "failed": "❌",
        "declined": "⛔",
    }.get(tool.status, "ℹ️")
    status = {
        "inProgress": "in progress",
        "completed": "completed",
        "failed": "failed",
        "declined": "declined",
    }.get(tool.status, tool.status)
    facts = dict(tool.facts)
    compact: list[str] = []
    if "exit_code" in facts:
        compact.append(f"exit {facts['exit_code']}")
    if "duration_ms" in facts:
        compact.append(_duration(facts["duration_ms"]))
    if "change_count" in facts:
        compact.append(f"{facts['change_count']} changes")
    if "receiver_count" in facts:
        compact.append(f"{facts['receiver_count']} agents")
    if facts.get("read_only") is True:
        compact.append("read-only")
    if "success" in facts:
        compact.append("success" if facts["success"] else "unsuccessful")
    if "result_count" in facts:
        compact.append(f"{facts['result_count']} results")
    if tool.call_truncated or tool.result_truncated:
        compact.append("truncated")
    suffix = " — " + " · ".join(compact) if compact else ""
    return (
        f"{icon} <strong>{html.escape(tool.label)}</strong> "
        f"{html.escape(str(status))}{html.escape(suffix)}"
    )


def _duration(value: object) -> str:
    if isinstance(value, int) and not isinstance(value, bool):
        if value < 1_000:
            return f"{value} ms"
        seconds = value / 1_000
        return f"{seconds:.1f} s"
    return f"{value} ms"


def _fenced(content: str, language: str) -> str:
    content = _expose_html_comments(content)
    longest = max((len(match.group(0)) for match in re.finditer(r"`+", content)), default=0)
    fence = "`" * max(3, longest + 1)
    safe_language = language if re.fullmatch(r"[a-zA-Z0-9_-]+", language) else "text"
    return f"{fence}{safe_language}\n{content.rstrip()}\n{fence}"


def _expose_html_comments(content: str) -> str:
    """Keep provider text visible instead of creating hidden GitHub regions."""

    return content.replace("<!--", "&lt;!--").replace("-->", "--&gt;")


def _chunk(body: str, turn_id: str) -> RenderedMirrorChunk:
    internal_key = "braid:turn:" + hashlib.sha256(
        turn_id.encode("utf-8")
    ).hexdigest()
    return RenderedMirrorChunk(
        index=0,
        count=1,
        body=body,
        body_digest="sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest(),
        ownership_marker=internal_key,
    )
