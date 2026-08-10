"""Safe GitHub comment rendering for a live turn projection."""

from __future__ import annotations

import base64
from dataclasses import asdict, dataclass
import hashlib
import json

from github_agent_bridge.turn_projection import TurnProjectionSnapshot


ACTIVE_VISIBLE_TEXT = "Agent 已看到，正在处理。"
DEFAULT_MAX_COMMENT_BYTES = 60_000


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
    if revision < 0:
        raise ValueError("revision must not be negative")
    if max_comment_bytes < 1_024:
        raise ValueError("max_comment_bytes is too small for a safe mirror")
    visible = _visible_text(snapshot)
    payload = {
        "items": [asdict(item) for item in snapshot.items],
        "raw_reasoning_items_excluded": snapshot.raw_reasoning_items_excluded,
        "revision": revision,
        "terminal_status": snapshot.terminal_status,
        "thread_id": snapshot.thread_id,
        "turn_id": snapshot.turn_id,
        "version": 1,
    }
    encoded = base64.b64encode(
        json.dumps(
            payload,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        ).encode("utf-8")
    ).decode("ascii")
    marker_id = hashlib.sha256(snapshot.turn_id.encode("utf-8")).hexdigest()[:24]

    one = _body(visible, encoded, marker_id, 1, 1, revision)
    if len(one.encode("utf-8")) <= max_comment_bytes:
        return (_chunk(0, 1, one, marker_id),)

    # Split both visible UTF-8 text and ASCII base64 under a conservative half
    # budget so no chunk can exceed GitHub's byte-oriented body boundary.
    content_budget = (max_comment_bytes - 512) // 2
    if content_budget < 1:
        raise ValueError("max_comment_bytes leaves no content budget")
    visible_parts = _split_utf8(visible, content_budget)
    encoded_parts = tuple(
        encoded[index : index + content_budget]
        for index in range(0, len(encoded), content_budget)
    ) or ("",)
    count = max(len(visible_parts), len(encoded_parts))
    chunks = []
    for index in range(count):
        body = _body(
            visible_parts[index] if index < len(visible_parts) else "",
            encoded_parts[index] if index < len(encoded_parts) else "",
            marker_id,
            index + 1,
            count,
            revision,
        )
        if len(body.encode("utf-8")) > max_comment_bytes:
            raise AssertionError("mirror chunk exceeded configured body limit")
        chunks.append(_chunk(index, count, body, marker_id))
    return tuple(chunks)


def _visible_text(snapshot: TurnProjectionSnapshot) -> str:
    status = snapshot.terminal_status
    if status is None or status == "inProgress":
        return ACTIVE_VISIBLE_TEXT
    if status == "completed" and snapshot.final_answer is not None:
        return snapshot.final_answer
    if status == "completed":
        return "Wrapper 状态：Agent turn 已完成，但没有可发布的最终回复。"
    if status == "interrupted":
        return "Wrapper 状态：Agent turn 在最终回复前被中断。"
    if status == "failed":
        return "Wrapper 状态：Agent turn 执行失败；任务结果未知。"
    return "Wrapper 状态：Agent turn 状态未知；未据此重试或宣称完成。"


def _body(
    visible: str,
    encoded: str,
    marker_id: str,
    chunk_index: int,
    chunk_count: int,
    revision: int,
) -> str:
    return (
        visible
        + "\n\n"
        + (
            f"<!-- agent-turn-mirror:v1:{marker_id}:chunk:{chunk_index};"
            f"count:{chunk_count};r:{revision}\n"
        )
        + encoded
        + "\n-->"
    )


def _chunk(
    index: int, count: int, body: str, marker_id: str
) -> RenderedMirrorChunk:
    return RenderedMirrorChunk(
        index=index,
        count=count,
        body=body,
        body_digest="sha256:" + hashlib.sha256(body.encode("utf-8")).hexdigest(),
        ownership_marker=(
            f"agent-turn-mirror:v1:{marker_id}:chunk:{index + 1}"
        ),
    )


def _split_utf8(value: str, max_bytes: int) -> tuple[str, ...]:
    if not value:
        return ("",)
    parts: list[str] = []
    current: list[str] = []
    current_size = 0
    for character in value:
        size = len(character.encode("utf-8"))
        if current and current_size + size > max_bytes:
            parts.append("".join(current))
            current = []
            current_size = 0
        if size > max_bytes:
            raise ValueError("one Unicode scalar exceeds mirror content budget")
        current.append(character)
        current_size += size
    if current:
        parts.append("".join(current))
    return tuple(parts)
