# Agent Session Interface

## Goal

Define the exact contract between the Agent Group and an `AgentSession`,
especially the meaning of `send_user_msg` and `reset_context_to`.

## Current Direction

- `AgentSession` has a provider/runtime-unique id, stable for its lifetime.
- `send_user_msg(new_user_msg, steering, reset_context_to)` is the high-level
  entry point used by the Agent Group.
- `new_user_msg` is a plain user message constructed by the Agent Group from a
  batch of events / event references. The session interface is event-agnostic;
  raw event refs do not cross this boundary.
- `reset_context_to` is the **new context content** (Markdown), not a revision id.
- `steering` is a boolean flag meaning "do not queue; deliver this user message
  to the running turn at the nearest safe point". It is used for urgent events
  such as `@braid` mentions. If no turn is running, the adapter starts one.
- `send_user_msg` **always returns an `AgentSession` instance** (which may or
  may not be the same logical instance as the caller held) and returns
  **immediately**. The caller uses the returned instance going forward.
- The caller does **not** inspect `status()` to decide whether to call
  `send_user_msg`; the session internally queues, steers, or waits for context
  replacement.
- `status()` returns the current `idle | running | failed` state synchronously,
  mainly for introspection/debugging.
- `status_stream()` (or equivalent async stream / watch channel) provides
  lifecycle changes to the Agent Group without polling.
- `recv()` is **not part of the core interface**. It may exist in
  adapter-specific implementations for debugging, but Braid core does not
  depend on it.

## Agreed Decisions

- **Adapter owns physical session replacement internally.** The caller holds
  one stable logical `AgentSession` instance; the adapter may fork or swap the
  underlying physical session transparently.
- **`send_user_msg` returns immediately** with a (possibly new) session
  instance; the Agent Group consumes async lifecycle changes via `status_stream()`.
- **`steering` means urgent delivery**: skip internal queueing and reach the
  current or next turn as soon as the adapter allows.
- **Reset waits for turn completion.** When `reset_context_to` is set and a turn
  is running, the adapter waits for that turn to reach a terminal state (or
  safely interruptable boundary), then applies the context reset and starts the
  next turn. The caller invokes `send_user_msg` only once.
- **`recv()` is not a core requirement.** Braid core only needs session
  lifecycle/health. `status()` is synchronous; `status_stream()` is the primary
  async signal.

## Open Questions

1. Should the async signal be a `futures::Stream<SessionStatusEvent>`, a
   `tokio::sync::watch::Receiver<SessionStatus>`, or something else?

## Pending Decision

Choose the concrete async signal mechanism.

## Recommendation

Use a `tokio::sync::watch::Receiver<SessionStatus>` for the core contract.

- `watch` gives every consumer the *latest* state immediately on subscribe, and
  emits only when the state changes. This is exactly what an Agent Group needs
  to decide when to send the next batch.
- It is cheap, single-producer/multi-consumer, and already used elsewhere in the
  Braid runtime (`health` watcher, `shutdown` channel).
- A `Stream` of deltas is more powerful but unnecessary if the only states are
  `idle|running|failed`; a consumer can derive "turn completed" by observing a
  transition from `running` to `idle`/`failed`.
- If we later need richer lifecycle events (token usage, turn IDs, errors), we
  can replace `SessionStatus` with `SessionState { status, last_error, turn_id }`
  without changing the channel shape.
