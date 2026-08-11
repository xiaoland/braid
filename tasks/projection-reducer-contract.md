# Projection Reducer Contract

This task-local contract defines how the Wrapper derives a bounded GitHub turn
projection from Codex app-server events. It is not a provider transcript,
thread-recovery format, semantic task model, source of Agent truth, or remote
serialization format. GitHub presentation is owned by the linked
[Human-readable turn mirror](human-readable-turn-mirror.md).

## Authority and Scope

- The pinned authority is the generated stable v2 schema for
  `codex-cli 0.147.0-alpha.6.5`, digest
  `7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb`.
- app-server owns thread and turn history. GitHub owns the published comment.
  Wrapper owns only a sanitized projection state until the corresponding
  terminal mirror revision is acknowledged.
- The reducer consumes one ordered stdio connection and scopes every event by
  exact `(threadId, turnId)`. A logical projection item is keyed by
  `(threadId, turnId, itemId)` and receives a stable first-seen sequence.
- Only three logical message kinds exist: `assistant_message`,
  `reasoning_summary`, and `tool_call`. A tool call contains a deterministic
  description, bounded schema-approved call payload, bounded schema-approved
  result, and bounded mechanical facts. Unknown item types and unknown fields
  never pass through as arbitrary JSON.
- Raw events are not projection-history entries and are not durably retained.
  The bounded sanitized logical messages may be checkpointed solely so a
  Wrapper restart can converge the existing GitHub mirror. They are deleted
  after the terminal mirror/outbox is acknowledged, leaving only delivery
  identity and digests.

## Common Reducer Rules

1. `item/started`, item-specific deltas, and `item/completed` mutate one stable
   logical item; they do not append three messages.
2. Delta text is provisional. `item/completed.item` is the authoritative item
   snapshot and replaces, rather than appends to, any locally accumulated
   equivalent field.
3. An exact duplicate started/completed snapshot is a no-op. Two different
   completed snapshots for one item ID are protocol drift and fail that item
   closed; the Wrapper does not choose one by arrival time.
4. Delta notifications have no delivery or sequence ID, so equal text is not a
   valid dedupe key. The live stdio connection supplies order; after reconnect,
   current provider item snapshots replace provisional text instead of replaying
   old deltas.
5. A started item or content delta makes the projection dirty but does not
   increment the completed-message count. The first authoritative completion
   increments the count once. Duplicate completion and terminal replay do not.
6. Stable ordering is first-seen order. Items discovered only in the terminal
   turn snapshot are appended in its item order. A later snapshot cannot reorder
   already published logical messages.
7. `turn/completed` first reconciles any previously unseen completed items from
   its turn snapshot, then closes the reducer and forces one publication. The
   terminal status never implies GitHub task success or failure.

## Assistant Message Lifecycle

| app-server input | Reducer action | Dirty/count effect |
| --- | --- | --- |
| `item/started` with `type=agentMessage` | Create or confirm the item using `id`; seed provisional `text` and optional `phase`. Exclude `memoryCitation` and unknown fields. | Dirty only when non-empty publishable text appears; count unchanged. |
| `item/agentMessage/delta` | Append `delta` to provisional text for `itemId`; create a provisional phase-unknown item if started was not observed. | Dirty; count unchanged. |
| `item/completed` with `type=agentMessage` | Replace provisional text with authoritative `item.text`; set authoritative `phase`; mark completed. | Dirty; increment once on first completion. |
| `turn/completed` | Reconcile missing agent messages from `turn.items`; choose visible final only under the rule below. | Force terminal publication. |

`phase` has two schema values and one compatibility state:

- `commentary`: retain as a visible chronological Agent activity message.
- `final_answer`: retain as an assistant message and a final-answer candidate.
  A steer may create multiple candidates; if and only if the turn status is
  `completed`, the last completed explicit candidate becomes the visible final
  response. Earlier candidates remain chronological activity.
- `null`/absent: retain as visible `phase_unknown` activity. Do not guess
  commentary or promote it to visible final. The pinned protocol probe must continue proving
  explicit final phases; a provider that loses this guarantee requires a new
  version-specific compatibility decision.

An in-progress assistant delta may appear in a maximum-wait publication as one
partial logical message. Completion later replaces it in place; it never creates
a second message merely because the authoritative text arrived.

## Reasoning Summary Lifecycle

| app-server input | Reducer action | Dirty/count effect |
| --- | --- | --- |
| `item/started` with `type=reasoning` | Create item and seed only `summary[]`; discard `content[]`. | Dirty only for non-empty summary; count unchanged. |
| `item/reasoning/summaryPartAdded` | Ensure the indexed summary part exists; publish no text for an empty slot. | No effect until text exists. |
| `item/reasoning/summaryTextDelta` | Append `delta` to the exact `(itemId, summaryIndex)` part. | Dirty; count unchanged. |
| `item/reasoning/textDelta` or `rawContentDelta` | Ignore completely; these are raw reasoning. | No dirty/count effect. |
| `item/completed` with `type=reasoning` | Replace provisional parts with authoritative `summary[]`; discard `content[]`; mark one completed reasoning-summary message. | Dirty; increment once. |

Multiple summary parts render as one ordered reasoning-summary message, not one
message per delta or part. Empty summaries are omitted and do not count.

## Tool-call Lifecycle

Tool events are reduced mechanically; Wrapper does not ask an LLM to summarize
them and does not interpret task meaning. `item/started` may expose a bounded
in-progress `<details>` projection through the maximum-wait trigger.
Schema-approved payload/result deltas update that same logical tool message;
they never become independent history entries. `item/completed` replaces
provisional fields with one authoritative terminal call/result snapshot where
available and increments the message count once. Raw CoT and unknown progress
events never enter the projection.

| item type | Human-visible call/result projection | Always excluded |
| --- | --- | --- |
| `commandExecution` | summary: status/exit/duration; call: command/actions and relevant cwd; result: bounded stdout/stderr/aggregated output | process ID, environment, credential-bearing protocol metadata, unknown fields |
| `fileChange` | summary: status/change count; call/result: bounded paths and patches/diffs | unrelated workspace state and unknown fields |
| `mcpToolCall` | summary: server/tool/status/duration/read-only; call: bounded arguments; result: bounded result or error | app context, credential metadata, unknown fields |
| `dynamicToolCall` | summary: namespace/tool/status/success/duration; call: bounded arguments; result: bounded returned content | unknown fields |
| `collabAgentToolCall` | summary: provider tool/status/receiver count; call: bounded prompt; result: bounded provider-reported outcome | raw remote thread history, model internals, unknown fields |
| `webSearch` | summary: action/status; call: bounded query/action details; result: bounded returned evidence | unknown fields |
| `imageView` | summary: view/status; call: bounded file reference; result: provider-visible bounded metadata, not image bytes | image binary/base64 and unknown fields |
| `imageGeneration` | summary: status; call: bounded prompt; result: bounded provider result/reference | image binary/base64 and unknown fields |

For schema status enums, the summary preserves only provider-labelled
`inProgress`, `completed`, `failed`, or `declined` as applicable. A
schema-approved error body may appear in the folded result subject to the same
redaction and byte bounds; it is not copied into the short `<summary>` label.
Unknown tool-like types are omitted and recorded only as bounded local
diagnostics until the schema contract is reviewed.

## Publication Trigger State

The publisher maintains, per active turn:

- `dirty`: whether the sanitized projection differs from the last acknowledged
  GitHub revision;
- `completed_messages_since_publish`: first completions since that revision;
- `oldest_dirty_at`: local monotonic time when the current dirty generation
  began;
- `projection_digest`: digest of the complete sanitized render input.

Publication occurs when dirty and any of these conditions holds:

1. `completed_messages_since_publish >= message_count_threshold`;
2. `now - oldest_dirty_at >= maximum_dirty_age`;
3. authoritative `turn/completed` was received.

The first two conditions are `OR`, not `AND`; otherwise a low-volume long turn
can remain stale forever. `maximum_dirty_age` is deliberately a long liveness
bound, not a periodic edit cadence. A successful acknowledged edit resets the
count and dirty-age generation. Failed or uncertain publication resets neither.
An equal projection digest is always a no-op regardless of clocks.

The active placeholder comment may be created once when the turn starts. Its
creation is separate from the projection batching policy. Every revision is
visible Human-readable Markdown; no revision publishes hidden JSON, IDs, or an
ownership marker. Terminal publication promotes the explicit final assistant
response or shows the neutral provider/transport terminal state while retaining
the bounded visible activity timeline.

## Recovery and Boundedness

- A checkpoint contains only the three bounded logical message kinds,
  lifecycle, stable order, completion flags, schema-mapped tool call/result
  evidence, render digest, and publication counters. It contains no raw
  app-server frame, raw reasoning, arbitrary protocol item, GitHub comment
  snapshot, credential, whole environment, or worktree state.
- On Wrapper restart, provider-owned current-turn snapshots reconcile this
  checkpoint. Completed snapshots replace provisional fields; detail available
  only in lost deltas may disappear rather than be guessed.
- A terminal acknowledged mirror deletes the projection checkpoint. GitHub
  Human edits or deletion never cause the Wrapper to resurrect it from provider
  history.
- Independent byte and message-count ceilings apply before rendering. Reaching
  a ceiling omits further interim projection detail with one bounded truncation
  marker; it never shards arbitrary tool data. The explicit final assistant
  response has its own GitHub body-limit policy.

## Remaining Calibration, Not Architecture

- Choose initial `message_count_threshold` and `maximum_dirty_age` from a real
  bounded turn and notification-cost evidence. They remain run-scoped tuning
  values; changing them must not alter reducer semantics.
- Set per-message and per-turn text ceilings after measuring ordinary coding
  turns. Synthetic secrets and recursive GitHub-comment content must be included
  in the black-box negative campaign.
- Revisit provider-labelled plugin identifiers only if a concrete Human-facing
  need appears; the first reducer deliberately omits them.
