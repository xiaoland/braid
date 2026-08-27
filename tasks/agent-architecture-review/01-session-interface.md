# Agent Session Interface

## Goal

Define the exact contract between the Agent Group (`sessions`) and an
`AgentSession`, especially the meaning of `send_user_msg` and
`reset_context_to`.

## Contract (binding)

```rust
pub enum SessionStatus { Idle, Running, Failed }

pub enum TurnOutcome { Completed, Interrupted, Failed, Unknown }

pub enum SessionEvent {
    TurnStarted { turn_id: String },
    TurnTerminal { turn_id: String, outcome: TurnOutcome },
    SessionReplaced { old_id: String, new_id: String },
    Failed { reason: String },
}

#[async_trait::async_trait]
pub trait AgentSession: Send + Sync {
    fn id(&self) -> &str;
    fn status(&self) -> SessionStatus;
    fn events(&self) -> broadcast::Receiver<SessionEvent>;
    async fn send_user_msg(
        self: &Arc<Self>,
        msg: String,
        steering: bool,
        reset_context_to: Option<String>,
    ) -> Result<Arc<dyn AgentSession>, SessionError>;
}
```

## Semantics

- `msg` is a plain user message (Event Reference batch text) built by the
  caller; raw event refs never cross this boundary.
- `steering = true`: do not queue; deliver at the nearest safe point of the
  running turn (adapter uses `turn/steer` with its own tracked
  `expectedTurnId`; a non-steerable turn defers to the next boundary). If no
  turn is running, the adapter starts one.
- `reset_context_to = Some(context)`: the caller materialized Context and its
  revision advanced. The adapter fences the old revision, waits for the
  current turn to reach a terminal/safe boundary, replaces the physical
  session (Codex v1: fresh `thread/start` + `thread/inject_items`; inject
  cannot replace history), then starts the turn. The caller invokes
  `send_user_msg` exactly once.
- `send_user_msg` returns immediately with the logical session handle
  (usually the same `Arc`); physical replacement is adapter-internal.
- The caller never branches on `status()` before sending; queuing/steering/
  waiting is adapter-internal.
- `events()` is the core's only async signal. Turn terminal outcomes drive
  the Reaction Lifecycle and the group state machine; a bare status watch
  channel was rejected because it loses terminal reasons.
- `recv()` of conversation items is adapter-internal only (sampled telemetry /
  debugging). Braid never mirrors turn output.

## Adapter mapping (Codex v1, per app-server.md)

| Core call | Adapter action |
| --- | --- |
| `send_user_msg(m, false, None)`, idle | `turn/start` with `m` |
| `send_user_msg(m, true, None)`, running | `turn/steer` with tracked `expectedTurnId` |
| `send_user_msg(m, _, Some(ctx))` | fence → wait terminal/safe boundary → fresh `thread/start` + `thread/inject_items(ctx)` → `turn/start(m)` |
| disconnect mid-turn | reconnect + `thread/resume` if compatible (context/instruction/profile revisions, cwd, sandbox); else `Failed` |

## Session compatibility (TDD invariant 4)

A physical session is reusable only while context schema/revision, effective
instruction revision, profile revision, cwd, and sandbox all match. Any drift
⇒ fresh materialization path. The caller (sessions) decides by comparing the
freshly materialized revision against the session's recorded revision.

## Transport unknown

Disconnect without a terminal leaves the turn outcome `Unknown`: no parallel
turn, no terminal reaction, no retry of agent side effects. The adapter
reconnects/resumes; proven unavailability surfaces as `Failed` and the group
becomes `blocked`.
