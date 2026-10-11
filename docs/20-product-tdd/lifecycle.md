# Event and Session Lifecycle

This document owns the mechanical transition model from GitHub canonical
changes to Context replacement, provider turns, reactions, and shutdown.
Product interpretation remains with the Agent.

## Agent Group Lifecycle

Each Work Item/Profile pair has an Assignment Generation and one state:

| State | Meaning |
| --- | --- |
| `dormant` | No active Braid assignment/activation. |
| `materializing` | Profile, Context, the generation-scoped worktree, and physical Provider Session are being created. |
| `idle` | Session exists and no turn or pending Wake batch exists. |
| `debouncing` | Wake Events are accumulating against one quiet deadline/count. |
| `running` | One provider turn is active. |
| `reset_pending` | Current Context was invalidated; old output is fenced while interruption/terminal converges. |
| `sleeping` | Closed/unassigned lifecycle retained for possible reopen/reassignment policy. |
| `retired` | Merged PR or superseded generation; no automatic resume. |
| `blocked` | Complete Context, Profile, provider, worktree, or external authority is unavailable. |

Issue Activation starts a fresh generation, materializes Context, and creates a
physical session. A canonically confirmed native Agent App assignment ends in
`idle` and does not synthesize a turn. When that special GitHub capability is
unavailable, the first Trusted Braid Mention on a dormant Issue is both the
ActivationIntent and an urgent Wake reference, so the initial Issue turn starts
after materialization. PR Activation from a Trusted Braid Mention or `pr
ensure` likewise carries an explicit Wake reference. A successful `pr ensure`
is also the explicit implementation authorization for the initial PR turn; the
Implementation Agent must not wait for a second Human start message.

## Event Classification

Classification follows canonical before/after projection, not webhook action
names alone:

| Canonical change | Classification | Turn effect |
| --- | --- | --- |
| New external comment/review/review comment | Wake | Add reference; debounce/count. Add `eyes`. |
| Previously absent included scalar/set value added | Wake | Add reference; debounce/count. |
| Comment/thread unminimized or unresolved | Wake | Restored body enters Context; debounce/count. |
| PR synchronize, checks terminal, reviewer request change, or other excluded implementation fact | Wake reference | Does not enter Context; debounce/count. |
| Existing visible body/title/description edited or cleared | Hard Invalidation | Replace Context; continue only if a turn was interrupted. |
| Included scalar replaced/cleared or set member removed | Hard Invalidation | Same. Dedicated close/unassign rules take precedence. |
| Comment/review deleted or minimized; review thread resolved | Hard Invalidation | Remove body, retain lifecycle metadata, replace Context. |
| Direct Issue↔PR association graph changes | Hard Invalidation for affected PR groups; addition also Wake | Rebuild full direct graph. |
| Non-Agent edit to an open direct Associated Issue description | Cross-surface Hard Invalidation | Debounce edits, then interrupt/fence active PR and replace Context. |
| Other included direct Associated Issue change | Dependency Dirty plus Wake when Human-origin | No active PR interruption; rematerialize before next PR turn. |
| Closed Associated Issue body/comment change | No PR effect | Content is excluded until reopen. |
| Correlated Braid App Agent write, or write by the Profile's configured stable GitHub node identity | Agent-origin | Update canonical ledger; no wake/reset of originating group. |
| Direct write from any other/unconfigured identity | External | Normal classification; Braid cannot infer its origin. |
| HTML-comment-only, reaction-only, or timestamp-only change | No semantic event | Delivery evidence only. |

## Debounce and Urgent Mentions

Each group has one pending batch. A Wake Event starts/resets a 30-second Quiet
Window and increments the unsuperseded Wake count. The batch becomes runnable
when no Wake arrives for 30 seconds or the count reaches eight. Both values are
Profile-overridable. Lifecycle/invalidation refs can join the batch without
turning into Wake Events.

A Trusted Braid Mention is an exact configured handle in visible Markdown prose
from a current repository `MAINTAIN` or `ADMIN` actor. Code spans/blocks,
quotes, HTML comments, Braid App content, and less-privileged actors are
ordinary. Each `(comment ID, updatedAt/body version)` is consumed once.

- If idle/debouncing, it releases one immediate turn with the current batch.
- If a compatible turn is active, an edited/new mention sends one `turn/steer`
  Event Reference using the active provider turn precondition.
- If the turn is non-steerable or reset is pending, the mention remains urgent
  and is delivered at the first safe replacement/terminal boundary.
- Removing a mention cannot retract already delivered input.

## Hard Invalidation Sequence

```text
canonical diff
  -> advance Context Revision and fence old Braid-owned output
  -> state=reset_pending
  -> if active and interrupt supported: request interrupt
  -> wait for provider terminal/disconnect reconciliation
  -> create fresh physical Provider Session
  -> inject complete current GitHub Context
  -> if an active turn was interrupted: start one continuation turn with refs
     else: return idle unless the batch independently contains Wake/urgent input
```

Output emitted after fencing is retained in provider/telemetry evidence but is
not allowed to drive Braid-owned GitHub writes. Local code changes are not
rolled back; a replacement PR Agent inspects its dedicated worktree before
continuing.

The canonical Issue source retains the last completely observed Human-visible
description text once, independent of its Issue↔PR edges. An exact webhook body
edit, or a reconciliation root edit whose current visible description differs
from that source state, creates one PR-scoped Cross-surface Hard Invalidation
reference for every active direct edge and advances the source state atomically.
Title/label/project changes with the same visible body,
HTML-comment-only edits, closed Issues, inactive edges, and inactive PR Agent
Groups do not create this invalidation. The derived reference enters the PR's
ordinary Quiet Window/count batch; the Wrapper never infers semantic urgency
from the edited prose.

## Assignment and Terminal Lifecycle

- In native Agent App assignment mode, Issue unassignment starts/resets the
  same Quiet Window. Once settled, Braid safely interrupts an active turn,
  archives its physical session, and moves the generation to `sleeping`.
  Reassignment starts a new generation/session. Ordinary-App mention fallback
  does not fabricate assign/unassign lifecycle events.
- Issue/PR close or PR merge never interrupts the current turn. The lifecycle
  event grants exactly one Finalization Turn after the current terminal, or
  immediately when idle. Afterwards closed Issue and closed-unmerged PR groups
  sleep; merged PR groups retire.
- Reopen creates current Context, resumes with a fresh physical session when
  needed, and queues one normal debounced Wake turn.
- Delivery duplicates and events received while sleeping cannot grant a second
  Finalization Turn.

## Reaction State

All new external comments converge to Braid `eyes` after durable ingest. Only
the exact Trusted Braid Mention comment owns a turn cycle:

| Provider observation | Desired reactions on mention |
| --- | --- |
| accepted active turn | `eyes`, `rocket` |
| normal terminal | `eyes`, `+1` |
| confirmed unexpected terminal | `eyes`, `confused` |
| safely superseded by invalidation | `eyes` |
| transport/provider outcome unknown | `eyes`, `rocket` plus Operational Status Comment |

Ordinary turns never receive `rocket`, `+1`, or `confused`. Braid manages only
its own App reactions and never removes Human reactions. Reaction writes are
desired-state outbox operations; target deletion produces a tombstone, not a
replacement comment.

## Provider and Transport Unknown

连接丢失只证明旧执行结果未知，不能据此生成成功/失败 reaction。Core 将旧 turn
记为 Unknown 并发布 Operational Status。账本 fence 不能阻止旧执行通过 Git、GitHub
或 shell 继续产生副作用；替代执行启动前必须取得 adapter 的停止成功结果。停止失败
或无法确认时保留旧句柄和现场，将工作项阻断并公开原因，不重发结果未知的原生 RPC。
同一进程内已取得停止成功回执时，现有 Context reset 可以重建当前依据并重新调度
canonical Event References；这不保证旧执行的 GitHub、Git 或 shell 副作用只发生一次。
冷启动缺少原句柄时，遗留 starting、running 或 Unknown 也不能自动恢复。

Context reset 可以记录待物化状态，但只有旧执行停止成功才创建替代会话。Pi 先等待
原生 abort 收口再关闭 stdin 并等待进程退出；单独杀父进程不证明其工具已停止。
Codex 保留共享连接，等待目标 turn 的真实终态；连接丢失或通知缺失时不杀其它健康
会话来换取恢复。停止不能撤销已经发出的外部请求或收回脱离原生管理的后台服务。

## AgentSession Event Stream

The core signal between the adapter and the workers is the `AgentSession`
event stream, not raw provider notifications. Each active session yields a
`broadcast::Receiver<SessionEvent>`:

| Event | Meaning | Core reaction |
| --- | --- | --- |
| `TurnStarted { provider_turn_id }` | A new provider turn began. Exactly one per turn; the single authority for turn identity. | The dispatcher records `provider_turn_id` in the store and marks the turn started. |
| `TurnTerminal { provider_turn_id, outcome, error }` | A turn ended (`Completed`, `Interrupted`, `Failed`, `Unknown`). Exactly one per started turn; `error` carries the provider reason for `Failed`/`Unknown`. | The worker marks the turn terminal, updates reactions/status, and schedules the next batch. |

Delivery semantics are part of the contract:

普通 Failed 与 Unknown 都沿配置的 Operational Status surface 发布故障记录，包含
对应 turn、原生会话和具体原因。完整错误先存入 `turns.error`，再生成有界公开摘要；
公开内容移除配置中实际使用的 provider key，本地原始错误仍属于敏感运行材料。
数据库 schema 3 通过新增迁移保存该字段，旧的已发布迁移保持不变。

- **Exactly-once per fact.** One `TurnStarted`, then exactly one
  `TurnTerminal` — including session death, where the adapter synthesizes
  `Unknown` for the in-flight turn before going quiet. There is no second
  terminal-ish signal: provider turn errors travel inside `TurnTerminal`, and
  connection death is not an event at all (see below).
- **No subscription-timing gap.** The dispatcher subscribes *before* sending
  and hands the receiver to the drive loop inside `RunningAgentTurn`; the
  consumer never re-subscribes mid-turn.
- **失效范围由 adapter 决定。** `AgentProvider::closed()` 留在 adapter 内部。
  Core 使用句柄的 latched `is_unavailable()`，包括 idle 与晚订阅情形。
  一个句柄失效不触发全局 epoch，也不重建健康的同类会话。
- **持久化恢复兜底。** 恢复缺失/失效句柄之前，Group 将遗留的 starting/running
  turn 记为 Unknown 并阻断缺少停止依据的恢复。只向具备可用句柄的 opaque session ID 领取 runnable turn；
  不可用会话的输入保留待处理，不占住其他可用会话。

Responsibilities do not overlap:

- The **event queue** (scheduler plus store) decides which messages exist:
  debounce, batching, urgency, and retry — an unsent batch simply remains
  runnable.
- **`AgentSession`** only dispatches: idle, it starts a new turn; running and
  `steering`, it forwards input only to the expected active turn; running and
  not steering, it defers without accepting the message. It never queues;
  redelivery is the queue's job. `interrupt()` is
  the control-plane sibling of steering — an immediate operation on the
  observed in-flight turn that carries termination rather than input; the
  terminal still arrives via the event stream.
- **Group** 决定 Work Item 的物化、恢复、Context replacement、睡眠和退役，
  `SessionManager` 只索引中立句柄。替换完整 Context 不等于创建新的逻辑 Agent；
  adapter 的物理 ID 是可替换的执行绑定。
- **Adapter** 管理进程、连接、物理会话和通知翻译，不读取 GitHub、不操作业务 store。
  Group 释放旧句柄时，adapter 必须报告停止是否成功；失败不删除句柄、不启动替代执行。
  Codex 共享连接和 Pi 独立进程都服从同一契约。
