# End-to-End Acceptance

Braid is accepted only through real GitHub Work Items, the packaged Rust
binary, a real public webhook path, a real Codex app-server, and observable
Agent behavior. Unit/component/fake transport checks may diagnose code but do
not satisfy this contract.

## Boundary and Fixture

The campaign uses:

- a tagged, checksummed macOS arm64 Braid artifact installed into a clean
  directory without source, Python, or Cargo;
- the real `xiaoland/braid` repository or an approved disposable mirror at a
  pinned revision;
- a dedicated non-production Braid Agent App and webhook secret;
- a free Cloudflare Quick Tunnel to loopback ingress;
- the pinned real Codex app-server and a real Coding Agent login;
- one dedicated repository checkout for the Issue Profile and Braid-provisioned
  PR worktree;
- one maintainer actor and one actor without MAINTAIN/ADMIN permission;
- an OTLP endpoint suitable for retaining the campaign's full sensitive trace
  payload.

Drive the product only through GitHub, public `braid` CLI/health/status, process
control, provider executable, Git, installed artifact, and OTLP export. SQLite
inspection, internal Rust APIs, fake providers/GitHub, and hand-injected inbox
events are never pass oracles.

Acceptance helpers under [`../../scripts/tests/`](../../scripts/tests/) remain
operator scripts until the workflow stabilizes. GitHub objects and Human
verdicts are the product oracle.

## Campaign Setup

Create a real Issue whose description contains a distinguishable design fact,
one well-formed HTML comment containing a forbidden sentinel, and an ordinary
visible sentinel. Add labels and at least one available relationship/Project or
Development branch. Configure one `issue` Profile and one `pr` Profile with
different instructions that produce externally distinguishable behavior.

Set the campaign Quiet Window to 30 seconds, Wake count threshold to eight,
reconciliation interval to 60 seconds, and OpenTelemetry sample ratio to 1.0.
Record the installed Braid/Codex/Git/gh/Wrangler versions, config/profile
revisions, repository revision, App identity/permissions, process start time,
and pristine worktree states before activating Braid. Record whether the
installation exposes native Agent App assignment or uses the Trusted Braid
Mention fallback; the campaign must not claim one mode while exercising the
other.

## Required Journeys

### 1. Activation and Initial Context

If the installation exposes native Agent App assignment, assign it to the
Issue. Braid must create one Issue Agent generation and physical Codex thread,
inject complete current Issue Context, and remain idle without inventing a turn
or Agent comment. If it is an ordinary GitHub App, create one maintainer
Trusted Braid Mention on the dormant Issue instead. That same comment must
produce one generation and one initial turn after complete Context injection;
Braid must not fabricate an assignee or a second Wake. In both modes the
Context must:

- use plain `GitHub Issue: owner/repo#number` identity;
- include title, description, selected metadata/relationships, and visible
  comments in deterministic order;
- include the visible sentinel and exclude the HTML-comment sentinel;
- contain no JSON envelope, GraphQL node IDs, duplicate URL, Operational Status
  body, or folded body.

In native assignment mode, unassign and reassign once. Unassignment must wait
for debounce and stop the old generation; reassignment must create a new
generation/session, not resume stale provider memory. The ordinary-App fallback
does not claim this native lifecycle.

### 2. Debounce, Count, and Trusted Mention

Create one ordinary Human comment. It must receive `eyes` after durable ingest,
must not show `rocket/+1/confused`, and must not start before the 30-second quiet
deadline. The resulting Agent message is published by the Agent itself under
the configured identity and attribution.

Create eight distinct ordinary comments faster than the Quiet Window. They must
coalesce into one turn released by the count threshold. Then create:

- an unprivileged `@braid` comment;
- `@braid` inside code, quote, and HTML comment;
- one maintainer visible `@braid` comment.

Only the last bypasses debounce. It receives `rocket` after provider acceptance
and `+1` after normal terminal. Edit that mention after delivery; while the turn
is active the new version must steer the same expected turn, not create a
parallel turn. A controlled unexpected terminal replaces `rocket` with
`confused`. Ordinary turn failure instead updates Operational Status and never
adds a terminal reaction to its trigger comments.

### 3. Canonical Lifecycle and Context Replacement

With an Issue turn active, edit a visible comment and the Issue description,
then minimize one visible comment and delete another. Braid must fence the old
Context Revision, safely interrupt/settle the old turn, create a new physical
Codex thread, and inject current complete Context. The replacement must contain
new description/comment bodies, only metadata for the minimized/deleted
comments, and no stale body or provider transcript.

Repeat equivalent lifecycle on an idle group. Context/session is replaced but
no turn starts until a Wake Event. Unminimizing the comment restores its body and
creates a Wake Event. Older-after-newer webhook redelivery must not regress the
projection or create a duplicate turn.

### 4. Implementation Request and PR Profile

Have the Issue Agent publish a concise implementation request and call
`braid gh pr ensure` with that Issue comment ID twice concurrently. Exactly one
Draft PR, native association, ActivationIntent, PR Agent generation, PR Profile,
and dedicated worktree must result. When the selected branch initially equals
base, the PR contains exactly one Braid App bootstrap commit whose tree equals
its parent and whose message identifies the request comment; it changes no
file. A second distinct request comment may create a second PR; 1:N and N:1
associations must not be rejected.

The PR Agent's initial Context must contain all current directly Associated
Issues followed by the PR section. Open Issues are complete; a closed Issue is
metadata/reference only. It must omit head SHA, commit/diff/check summaries and
normally reviewers. The Implementation Agent must operate from the dedicated
worktree, create a real diff, verify it, publish concise PR comments itself, and
use ordinary Git/gh freely.

Edit an open Associated Issue description while the PR Agent turn is active.
After debounce it must Cross-surface invalidate and replace the PR session with
the new Issue design. Change a label/comment instead: it may schedule a PR turn
and must refresh Context before that turn, but must not interrupt the active PR
turn. Close the Issue: its description/comments disappear from future PR
Context while reference/state remain. Reopen restores full Context.

### 5. Origin and Working-Memory Maintenance

Use `braid gh` from each Agent to update its description/body and create one
attributed concise comment. The correlated webhook echo enters future canonical
Context but does not wake/reset the originating group. Confirm the quote-block
Profile/role attribution cannot be supplied or altered by the Agent body.

Then let the Agent perform an ordinary direct `gh` edit under its Profile's
configured stable GitHub actor. Braid must not block it, and the exact actor
match suppresses self-wake/reset. Repeat under an unconfigured/shared Human
identity: it remains allowed but is processed as a normal external change and
may wake/invalidate. This is the explicit autonomy/attribution tradeoff.

### 6. Close, Merge, Reopen, and Finalization

Close an active Issue and an active PR without merging. Current turns must not
be interrupted; each group receives at most one Finalization Turn, then sleeps.
Additional deliveries while closed grant none. Reopen and observe one current
Context plus one ordinary debounced turn.

Merge the PR in the controlled fixture. It receives at most one Finalization
Turn and then retires; later events do not reactivate it automatically.

### 7. Context Pressure

Run with a campaign-only low Context byte budget. At soft pressure Braid still
starts the complete turn and updates status. Above hard pressure, and separately
with an intentionally unavailable pagination/permission boundary, it must start
no provider turn and update one mutable `context-too-large` or
`context-unavailable` Operational Status Comment on configured surfaces.
No partial, truncated, or generated summary may reach Codex.

### 8. Synchronization, Restart, and Unknown State

Exercise duplicate delivery, temporary tunnel loss, 60-second reconciliation,
and an out-of-order edit. GitHub state must converge without duplicate logical
turns, reactions, status comments, or `pr ensure` PRs.

Restart Braid while debouncing, while a reaction/status write is uncertain, and
while a provider turn is active. Repository owner lease, batches, outbox, and
same compatible physical thread must converge without parallel turns. Disconnect
app-server without terminal evidence: Braid keeps the turn unknown, preserves
the trusted mention's `rocket`, and updates Operational Status rather than
claiming success/failure.

### 9. Telemetry, Migration, and Distribution

With sample ratio 1.0, the retained OTLP trace must correlate webhook receipt,
canonical reread, Context materialization, scheduler/session/provider lifecycle,
GitHub writes, and terminal result. It must contain the controlled comment body,
summary, raw webhook payload, provider transcript/input/output, credential
sentinel, and local path sentinel. Large payloads must be log/event bodies rather
than metric labels. Metrics remain available independently of trace sampling.

Repeat a bounded run at ratio 0.10 and verify trace-level consistent selection:
a retained trace has its complete evidence and a dropped trace does not export
orphan spans. Enable incident mode at 1.0 and verify all new root traces export.

Install a prior schema artifact, create its DB, upgrade with the candidate, and
restart. Verify the forward-only migration ledger/checksums and retained
binding state. A binary declaring the DB schema too new must refuse startup.
Perform one declared compatible binary rollback; for an incompatible fixture,
restore the pre-migration backup rather than executing a down migration.

## Timing and Result Model

- ordinary one-event turn: no earlier than 30 seconds after the latest Wake and
  observable by 45 seconds absent recorded GitHub/provider outage;
- eight-event threshold or trusted mention: provider acceptance/reaction by 15
  seconds;
- reconciliation-only missed event: observable processing by 105 seconds with
  a 60-second interval;
- unassignment and Cross-surface description invalidation: debounce before
  retirement/interruption.

Each assertion is `pass`, `fail`, or `unavailable`. `Unavailable` never counts
as pass. Public evidence includes Work Item/comment/reaction IDs and timestamps,
App/actor identity and permissions, native associations, Draft/Ready/lifecycle,
refs/diff/checks, packaged binary/version/checksum, process control, and Human
verdicts. OTLP and provider evidence may prove Context/telemetry-specific
assertions but cannot substitute for public product behavior.

The golden Issue-to-PR journey must pass without corrective operator action. A
fresh installation and fixture must pass the entire campaign three consecutive
times before the implementation PR becomes ready.
