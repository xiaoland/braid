# Topology Audit

Method: enumerate every object (node), its owner, and every reference/flow
(edge) with direction; then check each packet decision against the topology and
against the product authorities (`docs/10-prd/*`, `docs/20-product-tdd/*`).

## Nodes (objects) and owners

| Node | Owner | Authority |
| --- | --- | --- |
| GitHub canonical state | GitHub | bodies, metadata, comments, lifecycle, identity |
| Webhook / GraphQL ingress | Braid `github` | verified durable ingest |
| Canonical ledger | Braid `store` | object versions, tombstones, dedupe |
| Event (classified) | Braid `events` | canonical diff classification |
| Quiet-window batch | Braid `scheduler` + `store` | coalescing, single-flight claim |
| Agent Group (generation) | Braid `sessions` | lifecycle, fencing, finalization |
| AgentSession (logical) | Braid core trait | stable caller-facing handle |
| Provider session (physical) | Provider | thread, turns, resume availability |
| Agent Runtime Adapter | Braid (compiled in) | `adapter_type` + contract version |
| Agent Runtime | User's machine | codex app-server, pi, HTTP harness |
| Connectivity config | Braid worker registry | adapter-defined shape |
| Agent Profile | Braid config | versioned, immutable, digest-identified |
| LLM provider entry | Braid config | protocol, key ref, models, costs |
| Skills / MCPs | runtime home | resolved by adapter at session start |
| Context | Braid `context` | fresh materialization before every turn |
| Worktree | Braid `worktree` | per PR profile+generation |
| Write outbox | Braid `writer` | Braid-owned GitHub mutations |
| Telemetry | Braid `telemetry` | sampled full-fidelity evidence |

## Edges (references and flows)

```text
GitHub --webhook/poll--> ingress --canonical diff--> events (classification)
events --> scheduler_batches (per work-item x profile)  [store]
scheduler --batch ready--> sessions (Agent Group)
sessions --materialize Context--> context projector (fresh from GitHub)
sessions --(msg, steering, reset_context_to)--> AgentSession
AgentSession ==logical handle==> adapter instance
adapter instance --connectivity config--> Agent Runtime
profile (adapter_type+version) --> adapter class        [locates]
profile (provider+model) --> llm_providers entry        [resolves]
profile (skills/mcps) --> runtime home contents         [via adapter]
adapter --SessionEvent stream--> sessions (reactions, state machine)
agent --braid gh--> writer outbox --> GitHub --> origin-correlated echo
```

Dependency direction is strictly top-down; nothing below references config
objects above it. Profile never carries connectivity config (including
`CODEX_HOME`/`PI_HOME`-style homes), because profile
user_instructions/skills/mcps are implemented against one fixed runtime home.

## Defects found in previous packets

| # | Defect | Fix |
| --- | --- | --- |
| T1 | My event taxonomy drifted from product/code: `UrgentMention`/`AgentOrigin` as types; missing `dependency_dirty` | Adopt store/lifecycle taxonomy exactly: classification ∈ {`wake`, `hard_invalidation`, `cross_surface_invalidation`, `dependency_dirty`, `lifecycle`} plus separate `origin` ∈ {`agent`, `external`}; trusted mention is an urgent **property** of a wake, not a type |
| T2 | `watch::Receiver<SessionStatus>` too thin: Reaction Lifecycle needs turn terminal status (`completed`/`interrupted`/`failed`/unknown) | Stream of `SessionEvent` (TurnStarted / TurnTerminal{outcome} / SessionReplaced / Failed); keep `status()` as sync snapshot |
| T3 | Renamed `tags`→`scopes` contradicts glossary "Profile Tag" and objects.md | Keep `tags` |
| T4 | Profile treated as plain mutable config | Profile is a versioned immutable snapshot with effective-config digest; resume compatibility = profile revision + instruction revision + context revision + cwd/sandbox (TDD invariant 4, app-server.md) |
| T5 | `reset_context_to` semantics under-specified vs Dependency Dirty | Caller materializes Context before every turn; passes `reset_context_to=Some` iff the materialized revision advanced. Adapter maps: None+idle→start_turn; None+running→steer (or queue at boundary); Some→fence/replace physical session (Codex v1: fresh thread/start+inject_items), then start turn |
| T6 | Invented parallel module names (producer/queue/group) | Map onto TDD modules: Event Producer ≡ `github` ingress + `events`; Event Queue ≡ `store.scheduler_batches` + `scheduler`; Agent Group ≡ `sessions`. Code keeps TDD names |
| T7 | `llm_providers` not defined by any product doc | Flag as Braid-internal extension; metadata-only this pass |
| T8 | Transport-unknown state unhandled | Disconnect ≠ terminal (lifecycle.md): adapter reconnects/resumes internally; core sees Running until proven failed → Failed; no parallel turn while outcome unknown |
| T9 | Worktree path unspecified | `<worker>/worktrees/pr-<number>/<profile>-g<generation>` per TDD |
| T10 | Single-flight invariant implicit | One active turn per group (MVP); enforced by scheduler claim + adapter refusing parallel starts |

## Corrected AgentSession contract

```rust
pub enum SessionStatus { Idle, Running, Failed }

pub enum SessionEvent {
    TurnStarted { turn_id: String },
    TurnTerminal { turn_id: String, outcome: TurnOutcome }, // Completed|Interrupted|Failed|Unknown
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

`recv()` of conversation items stays adapter-internal (telemetry/debug only);
Braid never mirrors turn output (invariant 7).
