# End-to-End Acceptance

Braid is accepted only through a real Issue-to-Draft-PR collaboration. Unit,
component, fake-transport, database, internal-log, provider-completion, and
health results may diagnose a run but never substitute for the external oracle.

## Boundary and Fixture

The campaign uses the real `xiaoland/braid` repository or an approved disposable
mirror of the same pinned revision, a genuine backlog Issue with merge intent,
real GitHub, the real Braid process, a real Cloudflare-backed webhook path, a
real Codex app-server/Coding Agent, and the Agent's real `gh` actions. Braid and
the candidate implementation run from separately pinned worktrees, branches,
runtime directories, and process identities. The primary Human worktree is
never a provider worktree.

Use separate Wrapper App and Agent identities, non-production credentials, two
trusted Human actors where the journey needs independent semantic review, and
one untrusted actor for urgent-hint policy. Drive Braid only through public CLI,
health, process, GitHub, and provider boundaries. Never inject an event below
GitHub, replace the Agent, inspect SQLite as a pass oracle, or use private logs
as evidence of success.

Acceptance helpers live in [`../scripts/tests/`](../scripts/tests/) while the
workflow is still being learned. They are operator tools, not accepted product
tests, until repeated campaigns stabilize their inputs and evidence contract.

## Required Journeys

1. **Golden collaboration**: Humans discuss an unresolved real Braid change; the
   Agent waits or discusses rather than treating comments as commands, then
   naturally creates a natively linked Draft PR in its dedicated worktree,
   implements, verifies, responds to review through the same thread, records
   material design changes on the Issue, and readies the PR only after evidence.
2. **Settling and mixed surfaces**: ordinary events reset quiet and coalesce;
   trusted `@agent` starts promptly, untrusted/duplicate hints do not multiply
   turns, active input steers one turn, and Issue/PR participation produces one
   canonical response plus deduplicated link-only FYIs.
3. **Canonical lifecycle**: controlled create/edit/delete/minimize, review
   dismissal, diff-comment lifecycle, and review-thread resolve/unresolve are
   reflected from current GitHub state; superseded requirements do not continue
   driving work.
4. **Synchronization**: normal webhook, repeated delivery, temporary tunnel loss,
   canonical reconciliation, and older-after-newer arrival converge without
   duplicate logical turns or mirror comments.
5. **Process continuity**: restart Braid with pending and active work and
   disconnect/resume app-server. Transport/outbox state converges without
   replacement threads or repeated Agent-owned side effects; unknown provider
   state never masquerades as completion or failure.
6. **Turn projection**: one stable comment progresses by logical-message count or
   maximum dirty age and terminates immediately. Raw and rendered bodies satisfy
   [`turn-projection.md`](turn-projection.md), including tool details, bounds,
   omission, no-op suppression, and forbidden-content assertions.

## Timing and Result Model

Acceptance configuration uses a 30-second quiet window: ordinary processing must
not appear before 30 seconds after the last relevant event and should appear by
45 seconds absent a recorded GitHub outage. Trusted `@agent` should expose
processing within 15 seconds. Canonical reconciliation runs every 60 seconds and
missed webhook state should expose processing within 105 seconds.

Each mechanical assertion is `pass`, `fail`, or `unavailable`, backed by public
object URLs/IDs, timestamps, bodies, refs/SHAs, checks, actor permissions, and
process-control observations. Each trusted Human independently records whether
premature action, settled interpretation, steering, and final evidence are
acceptable. `Unavailable` never counts as pass.

The golden Braid Issue-to-PR journey must pass without corrective operator
intervention. Before the source PR is ready, a clean candidate installation with
a fresh binding must pass the full adversarial campaign three consecutive times.
Dogfood on the source binding and clean-candidate campaigns are distinct
evidence and cannot substitute for each other.

## Evidence Bundle

Retain fixture identities and pinned versions, Issue/PR links and native
association snapshots, comment IDs plus raw/rendered bodies over time, available
webhook delivery IDs, actor permissions, refs/SHAs, Draft/Ready transitions,
review/thread/check state, process stop/start timestamps, protected-worktree
snapshots, and both Human verdicts. Private Braid storage, provider transcript,
and internal logs may diagnose failure but remain outside the pass oracle.
