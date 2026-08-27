# Event and Session Lifecycle

This document owns the mechanical transition model from GitHub canonical
changes to Context replacement, provider turns, reactions, and shutdown.
Product interpretation remains with the Agent.

## Agent Group Lifecycle

Each Work Item/Profile pair has an Assignment Generation and one state:

| State | Meaning |
| --- | --- |
| `dormant` | No active Braid assignment/activation. |
| `materializing` | Profile, Context, worktree when applicable, and physical Provider Session are being created. |
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

Connection loss is not a provider terminal. While a turn outcome is unknown,
Braid does not start a parallel turn, apply a terminal reaction, or retry Agent
side effects. It reconnects/resumes the same physical session when compatible;
if the provider proves it unavailable, the group becomes `blocked` and Braid
updates Operational Status. Context replacement may create a fresh session
only after the old turn is terminal or fenced so its later output is ignored.

## AgentSession Event Stream

The core signal between the adapter and the runtime is the `AgentSession` event
stream, not raw provider notifications. The runtime receives a
`broadcast::Receiver<SessionEvent>` from each active session and reacts to:

| Event | Meaning | Runtime reaction |
| --- | --- | --- |
| `TurnStarted { provider_turn_id }` | The adapter accepted a new turn. | Record `provider_turn_id` in store; mark turn `starting`. |
| `TurnTerminal { provider_turn_id, outcome }` | A turn ended with a known outcome (`Completed`, `Interrupted`, `Failed`, `Unknown`). | Mark turn terminal, update reactions, schedule next batch. |
| `SessionReplaced { old_id, new_id }` | The adapter created a fresh physical provider session (e.g. after context reset). | Persist the new `provider_session_id` in store; update `SessionManager` key. |
| `Failed { reason }` | The adapter encountered an unrecoverable error. | Enter failure/recovery path; mark group `blocked` or `unknown`. |

The adapter internally owns queuing, steering, physical session replacement,
and context-reset compatibility. The caller only dispatches through
`AgentSession::send_user_msg(msg, steering, reset_context_to)` and consumes
the event stream for reactions.
