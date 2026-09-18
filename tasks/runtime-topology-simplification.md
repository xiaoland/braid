# Runtime Topology Simplification

- **Status**: active，设计已于 2026-09-18 获 Human 确认，开始实现。
- **已确认决策**：采用 §4.3 的共享 provider 连接及 supervisor；验收必须运行完整 Slice 3 campaign，smoke 不能替代。
- **Branch**: `refactor/runtime-topology` from `origin/main`.
- **Goal**: Reorganize the async runtime around *state authorities* instead of
  *timers/tasks*, so the code reads as the product's essential loop:

  ```text
  GitHub state changes -> durable working state -> turn decision
    -> worktree + session materialization -> agent turn -> agent writes
  ```

- **Non-goals**: no behavior/state-machine changes (event kinds, debounce
  policy, fencing, outbox semantics stay identical); no store/context module
  splits (`store/mod.rs`, `context.rs` sizes are a separate concern); no
  migration changes.
- **Verification**: `cargo fmt --check`, `cargo check --locked --all-targets`,
  `cargo clippy --locked --all-targets` after every step; black-box behavior
  guarded by the complete Slice 3 campaign before merge.

## 1. Essential loop vs. current code

The essential loop has five duties, each with exactly one authority:

| Duty | Authority | Current home |
| --- | --- | --- |
| Observe (webhook + reconciliation) | `producer` | OK: `webhook_handler`, `reconciliation_worker` |
| Decide (classification completion, debounce, claims) | `queue` + `store` | Split: `advance_scheduler` driven from `producer::event_worker`; mention trust resolution in `event_worker` |
| Materialize (Context, worktree, session) | `group` | OK, but duplicated per kind |
| Execute (turn lifecycle: start/steer/reset/terminal) | `group` | OK, but duplicated per kind |
| Converge writes (outbox) | `outbox`/`writer` | Hidden inside `producer::event_worker` + shutdown drain in `runtime` |

The crux is one root cause: **modules were cut along async-task boundaries
("who has a loop") rather than authority boundaries ("who owns the state
transition")**. A loop is a cheap implementation detail; state ownership is
the architecture. Because loops accreted duties by proximity, responsibilities
drifted to wherever a timer happened to exist.

## 2. Confirmed cruxes (user's three points, with evidence)

### 2.1 `event_worker` is a mislabeled misc timer loop

`src/producer/ingress.rs::event_worker` does three unrelated duties on one
250ms tick:

1. `store.advance_scheduler()` — a pure SQL transition
   (`pending -> runnable` when `quiet_deadline <= now`). This is queue/decide
   work with zero GitHub involvement.
2. Trusted-mention authority resolution — async classification completion
   requiring GitHub network, with exponential backoff. This gates scheduling
   of mention events, i.e. also decide-stage work.
3. `drain_one_write()` — write-outbox convergence. This is write-back work,
   the last stage of the loop, sitting in the *producer* module.

Consequences beyond naming: one sequential loop couples three failure domains;
a slow mention-resolution GitHub call delays both scheduler advancement and
outbox drain; the module name `producer` now contains write convergence, which
contradicts the dependency direction documented in the Product TDD.

### 2.2 Issue/PR worker skeletons are one algorithm written twice

`issue_agent_worker` (413 lines) and `pr_agent_worker` (584 lines) share a
byte-identical skeleton: boot gates -> connection-epoch loop {connect -> fresh
`SessionManager` -> resume -> drive} -> drive loop {select shutdown / closed /
turn events / tick; on tick: lifecycle -> context reset -> assignment
materialization -> start turn}. The turn-event consumption block (~100 lines)
is identical modulo log strings.

The genuine domain differences are small and enumerable: profile selection,
system-prompt builder, resume compatibility checks (PR additionally requires
`head_ref` and `work_item_kind == "pr"`), and assignment materialization
(Issue: unassign settle, linked-branch ref resolution, assignee confirmation;
PR: head-repository check). These belong behind a policy/spec parameter, not
in two functions.

Drift has already begun: after a successful resume, `issue_agent_worker` sets
`health.provider = "connected"` and clears `last_error`; `pr_agent_worker`
does not. Every future lifecycle fix must be applied twice.

### 2.3 Provider connection ownership is misplaced

`connect_provider` is a *provider-scoped* resource: Codex is one app-server
process hosting many threads; Pi is a stateless supervisor spawning per-session
processes keyed by workspace. Nothing about it is Issue- or PR-specific. Yet
each group worker inlines the full epoch machinery (connect loop with 2s
retry, epoch-scoped `SessionManager`, resume convergence, reconnect surfacing).

Telling detail: the health snapshot already has a singular `provider` field,
and both workers write to it — the two "independent" owners race on shared
operator-visible state. The config is likewise singular
(`default_provider_config`). The per-worker connection duplication is an
accident of the worker-per-file layout, not a designed isolation boundary
(durable fencing already bounds the blast radius of any connection death).

## 3. Extended findings (beyond the three points)

- **Argument herds in `dispatch.rs`**: nearly every function takes
  `(store, github, config, provider, sessions, profile, profile_record, ...)`
  — 7-9 parameters. This is a missing `AgentGroup` context object; its absence
  is what makes the duplicated skeletons look "necessary".
- **`advance_scheduler` could eventually be a claim-time predicate**
  (`quiet_deadline <= now` inside claim queries) instead of a timer-driven
  stored transition. Deferred: it changes `runtime_status` observability
  semantics; recorded here as a follow-up candidate, not part of this task.
- **`runtime::serve` shutdown drain** duplicates outbox draining inline; with
  a dedicated outbox worker this stays but reads as the same duty's final
  flush.

## 4. Target design

### 4.1 Split `event_worker` by authority

- `queue_worker` (new, in `queue/`): `advance_scheduler` + trusted-mention
  authority resolution with its existing backoff. Both are decide-stage work;
  mention resolution is asynchronous classification completion.
- `outbox_worker` (in `outbox.rs`): owns the periodic `drain_one_write` loop.
- `event_worker` disappears. `webhook_handler` stays synchronous ingest in
  `producer::ingress`. `producer` returns to its documented duty:
  observe -> ingest.

### 4.2 One group worker, parameterized by kind

- Introduce `GroupKind` (`Issue` | `Pr`) and a `GroupSpec` carrying the real
  deltas: profile selection, system-prompt builder, resume-compatibility
  checks, assignment materialization entry point.
- One `agent_group_worker(spec)`: boot gates -> epoch subscription -> drive
  loop. The turn-event consumption block exists exactly once.
- `issue_agent.rs`/`pr_agent.rs` shrink to kind-specific materialization +
  spec definitions; the lifecycle skeleton moves to one shared driver.

### 4.3 Provider connection epochs owned by a supervisor, not by workers

- New `provider supervisor` (lives next to `provider/` or in `group/provider`):
  owns the connect/reconnect loop and publishes the current epoch
  `(epoch_id, Arc<dyn AgentProvider>)` over a `watch` channel; single writer
  of `health.provider`.
- Group workers subscribe: on epoch change they fence the in-flight turn
  (existing durable fencing), rebuild the epoch `SessionManager`, resume, and
  drive — using exactly today's resume/fence logic, relocated not rewritten.
- Consequence: one provider connection serves both group kinds. Blast radius
  of a connection death widens from one kind to both, but recovery is the same
  automatic reconnect and correctness is carried by durable fencing, not by
  process isolation. **Human 已确认共享连接的资源拓扑变化。** 未采用的备选方案 C': keep one
  connection per kind but extract the shared epoch-loop helper, removing the
  duplication without changing process topology.

### 4.4 `AgentGroup` context object

Bundle `(store, github, config, profile, profile_record, kind)` plus the
per-epoch `(provider, sessions)` into a struct; `dispatch.rs` free functions
become methods. No logic change; kills the argument herds and makes the drive
loop readable as a sequence of named steps.

### 4.5 Docs

Update `docs/20-product-tdd/README.md` module table (producer/queue/outbox/
group rows, provider-epoch ownership row in the state-authority table) to match
the realized topology.

## 5. Linear implementation plan

Each step compiles clean and passes fmt/clippy on its own; commit per step.

1. **Outbox worker**: move the drain loop out of `event_worker` into
   `outbox::outbox_worker`; spawn it in `runtime::serve`; keep the shutdown
   flush.
2. **Queue worker**: move `advance_scheduler` + mention-authority resolution
   into `queue::queue_worker`; delete `event_worker`; `producer` exports only
   ingress/reconcile.
3. **Provider epoch supervisor**: implement the supervisor + `watch` epoch
   channel (confirmed design 4.3). Rewire
   both workers to consume epochs; supervisor becomes sole `health.provider`
   writer.
4. **Unify group worker**: introduce `GroupKind`/`GroupSpec` and the shared
   epoch-drive skeleton; `issue_agent.rs`/`pr_agent.rs` keep only kind deltas.
5. **`AgentGroup` context + dispatch methods**: convert `dispatch.rs` free
   functions to methods on the context object.
6. **Docs**: update Product TDD module/authority tables.
7. **Final verification**: fmt/check/clippy + 完整 Slice 3 campaign；记录真实验收证据。
   PR 发布和 push 另按授权处理。

## 6. 交接与执行记录

2026-09-18：Human 确认设计，授权先提交任务计划，再开始实现，并要求完整 Slice 3 campaign。基线为 `e27cf7f`；确认时没有源码改动或已执行的本任务验收。旧的三份 closed/pivoted packet 不影响本任务，暂保留。
