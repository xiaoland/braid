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
public CLI, verifies schema 0→5, schema 1→5 with a pre-v5 backup, and
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
version/SHA-256, requires schema 5, and starts that exact packaged
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
