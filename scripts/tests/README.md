# Acceptance Helpers

This directory contains operator scripts that exercise public boundaries while
the real Braid workflow is being stabilized. A helper may invoke the installed
`braid` binary, GitHub/`gh`, Git/Codex/Wrangler, loopback health, OTLP, and
process controls. It must not import Braid internals, inject SQLite rows/events,
or replace GitHub/provider with fakes and then claim product acceptance.

Scripts should capture bounded machine-readable evidence and leave Human
verdicts explicit. Running a helper is not acceptance: every release candidate
must satisfy [`docs/10-prd/acceptance.md`](../../docs/10-prd/acceptance.md)
through real GitHub Work Items and a clean packaged installation.

`00_clean_install.sh` is the Rust foundation gate. It unpacks the release
artifact, scrubs Python/PDM/Cargo from the binary's `PATH`, exercises only the
public CLI, verifies schema 0→11, schema 1→11 with a pre-v11 backup, and
schema-newer refusal, and uses the adjacent
bounded OTLP/HTTP capture helper to observe sampling. Its direct SQLite write is
limited to constructing declared migration-compatibility fixtures; it does not
inject product events or count as workflow acceptance.

`10_context_projection.sh` is the Slice 1 real-object gate. It requires an
absolute App-backed config, a controlled Issue and PR, plus explicit fixture
expectations for visible/filtered/folded/deleted/paginated evidence and the
number of directly Associated Issues/PRs. It lowers the public diagnostic
GraphQL page size to one, proving the real page walkers without manufacturing
hundreds of comments, and checks that a closed Associated Issue contributes no
body. Missing fixture inputs return
`UNAVAILABLE` rather than substituting a user token, mock server, or synthetic
snapshot. The helper drives only packaged `braid` commands and compares the
emitted bytes.

`20_ingress_scheduler.sh` is the Slice 2 transport gate. It requires the
dedicated GitHub App to subscribe to Braid's declared Issue and PR event set,
an acceptance config, an App-matching `BRAID_WEBHOOK_SECRET`, authenticated
Human `gh`, and the packaged candidate binary. It creates disposable real
Issues, lets Braid own a Quick Tunnel only while the helper runs, and proves
durable webhook admission, App-owned `eyes`, delivery rededuplication,
older-after-newer non-regression, the 30-second debounce window, the
eight-event release threshold, trusted visible `@braid`, canonical
reconciliation during tunnel loss, pending-batch restart, and graceful webhook
URL restoration. It observes only public CLI/health/GitHub surfaces and never
starts a provider turn.

The default `BRAID_TEST_WEBHOOK_MODE=app` also proves App webhook handoff and
restoration. When GitHub has no App webhook object yet and its settings UI
cannot bootstrap one, the bounded `repository` mode can prove the transport
behavior without recreating the App. Start a Quick Tunnel to an unused local
ingress, then provide its `/webhook` URL through
`BRAID_TEST_PUBLIC_WEBHOOK_URL`; the helper creates and deletes one temporary
repository webhook and accepts optional `BRAID_TEST_INGRESS` and
`BRAID_TEST_HEALTH` loopback addresses. This mode does not prove App event
subscriptions or App webhook handoff/restoration, and its JSON verdict says so.

`30_issue_agent.sh` is the Slice 3 provider gate. It records the candidate
version/SHA-256 and starts that exact packaged
Braid binary against the real pinned Codex app-server, creates one temporary
repository webhook and disposable Issue, and uses the ordinary-App fallback:
the first trusted `@braid` activates the dormant Issue and starts the turn. It
then proves:

- one expected-turn edit steer and `eyes`/`rocket` to `eyes`/`+1` convergence;
- one ordinary turn only after the complete 30-second debounce window, with no
  request-style reactions;
- eight durably received events releasing one threshold turn, also without
  request-style reactions;
- real app-server process loss preserving an unknown turn and `rocket` while
  publishing one App-authored Operational Status Comment;
- a separate accepted turn using an intentionally unsupported model receiving
  a real Codex `turn.failed`, converging from observed `rocket` to `confused`;
- Agent-authored attributed comments, one session per fixture, and zero Braid
  turn-mirror comments.

The count timing begins only after all eight `eyes` acknowledgements prove
durable Braid receipt; GitHub webhook delivery latency is not mislabeled as
scheduler latency. The helper deletes its temporary webhook and closes both
Issues unless
`BRAID_TEST_KEEP_FIXTURES=1`.

Run it with a real authenticated Agent `gh` identity and an acceptance config
whose Codex home already has provider authentication:

```shell
BRAID_CONFIG=/absolute/path/to/braid.toml \
BRAID_BIN=/absolute/path/to/braid \
BRAID_TEST_WRANGLER=/absolute/path/to/wrangler \
BRAID_WEBHOOK_SECRET='acceptance-secret' \
scripts/tests/30_issue_agent.sh
```

This gate does not prove native Agent App assignment, App-webhook bootstrap,
Context invalidation/replacement, restart/resume, PR Agents, or the full release
campaign.

`40_context_lifecycle.sh` is the Slice 4 lifecycle gate at the public boundary.

It requires the current schema and a real App/Codex/Quick Tunnel fixture. A
plain external Issue-description edit while the Agent is idle must create a new
physical provider session without starting a turn. The same edit during an
accepted turn must fence and interrupt that turn, remove its `rocket` without
claiming success/failure, inject the complete current Issue Context into a third
session, and run one continuation that demonstrates the edited description was
seen. The helper then folds a visible comment and proves reconciliation replaces
the idle session while retaining only folded metadata; restores it and proves
one ordinary Wake uses the same valid session and sees the restored body; then
deletes another comment and proves a body-less tombstone plus a fifth physical
session. The fixture uses a real design-review message to create the active turn,
not a shell/sleep command. The helper observes only GitHub, loopback health,
`braid context`, and `braid status`; the status command exposes durable
reset/session/revision evidence rather than requiring SQLite inspection.

The same fixture now closes the Issue only after public status proves a turn
is running. That turn reaches its ordinary terminal without interruption; one
and only one Finalization Turn follows, then the Agent Group and provider
session become `sleeping`. A comment created while closed receives durable
`eyes` but grants no second turn. Reopen creates a fresh physical session from
the complete current GitHub Context and releases exactly one ordinary Wake
after the normal debounce window. Public status exposes the bounded
finalization count and terminal lifecycle so the helper never reads SQLite.

The helper also terminates the idle app-server child and requires Braid to
reconnect, resume the exact same provider thread, expose the resume through
public status, and process the next ordinary Wake after the debounce window.
Two additional fresh-runtime fixtures exercise the Profile pressure policy:
soft pressure publishes one Operational Status Comment but supplies the full
Context and completes a turn; hard pressure publishes one status, starts no
provider session or turn, and never truncates or summarizes the Context.

This Slice 4 helper does not yet claim native unassignment, active-turn
app-server loss, Context-unavailable recovery, or PR Agent coverage.

`50_issue_to_pr.sh` is the growing Slice 5 public gate without claiming the full
Issue-to-implementation journey. It creates a disposable real Issue, publishes
one App-authored attributed Implementation Request through `braid gh`, verifies
its durable receipt, and invokes `braid gh pr ensure` twice concurrently. It
requires one deterministic branch, one same-tree bootstrap commit, one Draft PR,
one native Issue association, one PR Profile/session, and one generation-scoped
worktree provisioned from a fresh source clone. The real Codex Implementation
Agent must then push one exact requested file and publish one concise attributed
PR comment. It then uses a real inline review comment and review-thread
resolution to prove PR Event References plus an idle PR Context replacement,
starts a second real PR turn, and edits the directly Associated Issue
description to prove debounced active-turn fencing, one continuation, and
preservation of the same dedicated worktree. A temporary free Quick Tunnel and
repository webhook provide the low-latency ingress; reconciliation remains the
60-second repair path. The PR must contain no Braid turn mirror. Cleanup deletes
the webhook, closes the PR and Issue, deletes the fixture branch, and removes
the isolated runtime/source.
Set `BRAID_TEST_KEEP_FIXTURES=1` to retain evidence intentionally.

The helper requires the installed Braid App to expose `Issues: write`, `Pull
requests: write`, and `Contents: write` and a schema-current config/private key:

```shell
BRAID_CONFIG=/absolute/path/to/braid.toml \
BRAID_BIN=/absolute/path/to/braid \
BRAID_TEST_WRANGLER=/absolute/path/to/wrangler \
BRAID_WEBHOOK_SECRET='temporary-repository-hook-secret' \
scripts/tests/50_issue_to_pr.sh
```

Its PASS is deliberately bounded. The expanded campaign covers external
direct-`gh` origin, one close/sleep/reopen cycle, merge retirement, compatible
idle PR provider/worktree resume, and a separate active Issue turn that becomes
one neutral `unknown` after Braid restart. A Profile configured from the start
with a distinct stable Agent GitHub identity and native App assignment remain
separate capability gaps; changing Profile identity under a running assignment
correctly fails compatibility instead of silently rebinding it.

`60_operations.sh` is the Slice 6 distribution and operations gate. It requires
the real acceptance config, matching App webhook secret, current candidate, and
a declared schema-compatible prior binary. Through public CLI/process/health
boundaries it packages and clean-installs the candidate, verifies upgrade and
compatible rollback, exercises graceful SIGTERM, forced-exit owner fencing and
post-expiry recovery, kills the runtime-owned Quick Tunnel and requires
automatic App webhook repair, and measures trace-consistent 10% sampling plus
incident-mode 100% sampling against a bounded OTLP receiver. Active-turn
unknown is deliberately composed with the real Slice 5/6 campaign rather than
manufactured in the local operations helper.

If the account-less Cloudflare service is externally unavailable, setting
`BRAID_TEST_SKIP_TUNNEL=1` runs the remaining independent operations journeys
and reports `tunnel=unavailable`; that result does not accept the tunnel
child-death/App-webhook-repair journey. A later unskipped run is still required.

```shell
BRAID_CONFIG=/absolute/path/to/braid.toml \
BRAID_BIN=/absolute/path/to/current/braid \
BRAID_PREVIOUS_BIN=/absolute/path/to/schema-compatible/braid \
BRAID_WEBHOOK_SECRET='dedicated-app-secret' \
scripts/tests/60_operations.sh
```
