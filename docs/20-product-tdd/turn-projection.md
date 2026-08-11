# Turn Projection Contract

Braid reconstructs a bounded, ordered message history solely to project one
live Agent turn onto GitHub. It does not reconstruct the provider thread,
become a recovery transcript, or preserve arbitrary app-server events.

## Admission and Lifecycle

Only three logical message kinds enter the projection:

- `assistant_message`: provider `agentMessage` text. `commentary` stays in
  chronological activity; on a completed turn, the last completed explicit
  `final_answer` becomes the promoted final response. Unknown phase remains
  visible activity and is never guessed to be final.
- `reasoning_summary`: only provider-labelled reasoning `summary`. Raw reasoning
  `content`, text deltas, and raw-content deltas are discarded.
- `tool_call`: one schema-mapped call with a deterministic Human label, terminal
  status, bounded mechanical facts, bounded approved call text, and bounded
  approved result text.

`item/started`, item deltas, and `item/completed` update one stable logical item
in first-seen order. Deltas are provisional; the completed item snapshot is
authoritative and replaces equivalent accumulated text. Duplicate completion
is a no-op; a conflicting terminal snapshot or unknown shape fails that item
closed. `turn/completed` reconciles any supported terminal snapshots, fixes the
turn status, and forces publication. An error notification alone is not a
terminal turn.

The initial tool allowlist is command execution, file changes, MCP calls,
dynamic tool calls, delegated-agent calls, web search, image view, and image
generation. Each kind selects explicit fields from the pinned app-server schema.
Unknown items and fields are never serialized generically. Binary content,
opaque MCP metadata, whole environments, process/thread/item identifiers, and
structured credential/environment fields remain excluded.

## Human-readable Markdown

An active comment starts with a neutral “Agent 正在处理” state. Chronological
assistant text is rendered under **Agent**, provider summaries under
**Reasoning summary**, and every tool call as:

````markdown
<details>
<summary>✅ <strong>Command</strong> completed — exit 0 · 855 ms</summary>

**Call**

```shell
gh issue view 123 --repo owner/repository
```

**Result**

```text
state: OPEN
```

</details>
````

The renderer owns labels, icons, status text, compact mechanical facts, safe
fence length, and whitespace. It does not paraphrase provider-authored assistant
or reasoning-summary text. The GitHub body contains no HTML comment, hidden
payload, ownership marker, revision/digest, or raw protocol JSON.

On a completed turn with a publishable final answer, `## Final response` moves
that answer to the top and removes the same item from chronological activity.
Completed-without-final, interrupted, failed, and transport-unknown have
separate neutral presentations; none claims the GitHub task succeeded or failed.

## Bounds and Publication

Defaults are 8 KiB per tool call, 16 KiB per tool result, 256 KiB for the
in-memory projection, and 60,000 UTF-8 bytes for the GitHub comment. Field
truncation preserves a bounded prefix and suffix with a visible omission notice.
If the rendered comment is too large, oldest activity is omitted with a visible
count; Braid does not shard a turn across comments. An oversized final response
fails rather than being silently truncated.

Intermediate edits occur when either the completed logical-message count reaches
its configured threshold or the oldest dirty projection reaches its configured
maximum age. Delta count and a fixed high-frequency timer are not publication
cadences. Terminal state flushes immediately; unchanged projection digests cause
no edit.

One turn owns one canonical remote comment. Known comments update by exact remote
ID. Marker-free uncertain-create recovery requires one comment matching target,
normalized Wrapper author, exact intended body digest, and bounded creation
window; zero, multiple, or unavailable matches remain uncertain. A foreign edit
is a conflict. For a mixed-surface turn, other participating surfaces receive at
most one short FYI linking the canonical comment and never receive a copy of the
projection.

## Rationale and Security Boundary

The rejected design accumulated raw/delta protocol events, encoded a cumulative
snapshot, and repeatedly wrote shards on a short timer. When a GitHub-reading
tool returned existing mirror comments, the next projection recursively embedded
them, multiplying comments and notification volume. Braid therefore reduces
semantic logical items, bounds every publishable tool field, suppresses no-op
edits, and never treats transport frames as Human messages.

Markdown `<details>` improves reading but is not a secrecy or size boundary;
GitHub, email, API, exports, and audits still receive its content. Structured
secret-like keys are excluded, while general free-form secret interception is
deferred. External campaigns must use non-sensitive fixtures and must not call
this projection a confidentiality guarantee.
