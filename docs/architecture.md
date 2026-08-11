# Architecture

Braid is a local transport bridge between two independent authorities. It keeps
the data plane narrow so neither GitHub nor Codex app-server is reconstructed
inside the Wrapper.

```text
GitHub webhook -> Cloudflare Tunnel -> loopback ingress
  -> durable event inbox -> quiet/urgent scheduler
  -> bound Codex app-server thread

GitHub GraphQL reconciliation -> same durable event inbox

app-server turn items -> bounded semantic projection -> comment outbox
  -> one canonical GitHub comment + link-only FYIs
```

## Authority Topology

- **GitHub** owns repository, Issue, PR, comment, review, review-thread,
  association, identity, and permission state.
- **Codex app-server** owns provider thread data, compaction, active-turn
  lifecycle, execution, and resume semantics.
- **Coding Agent** owns interpretation, readiness, plans, code, verification,
  and authorized Git/GitHub actions.
- **Braid** owns exact Issue-to-opaque-thread binding, canonical event delivery,
  mechanical scheduling, turn projection, comment publication, and its own
  idempotency state. It stores no provider transcript and no semantic task
  status.

## Runtime Units

- `github_webhook` verifies raw HMAC input and normalizes only supported
  transport envelopes.
- `github_api` and `reconciliation` read canonical Issue and current natively
  associated PR state so missed, edited, minimized, deleted, dismissed, and
  resolved objects converge after webhook gaps.
- `store` owns one SQLite transport state with owner fencing, Issue/thread
  bindings, opaque surface routes, deduplicated events, scheduler state,
  mirrors, outbox operations, and sync cursors.
- `turn_controller` claims one binding's settled events, starts or steers one
  provider turn, and freezes the first locally observed participating surface
  as the canonical fallback. A future explicit provider publication target may
  replace that fallback without interpreting prose.
- `provider_adapter` sends Wrapper-origin application context containing only
  event/action/actor/object/surface refs and digests. It never injects a GitHub
  comment as if the Wrapper were the Human author.
- `turn_projection`, `mirror_render`, and `mirror_publisher` reduce the allowed
  provider items into visible Markdown and converge one comment through a
  durable outbox. Their detailed contract is
  [`turn-projection.md`](turn-projection.md).
- `runtime` supervises the loopback ingress, provider connection, reconciliation,
  owner lease, controller, optional Quick Tunnel, and bounded health surface.

## Scheduling and Routing

One active binding admits one Agent turn at a time. Ordinary canonical events
reset a quiet deadline. Trusted exact visible `@agent` creates an urgent hint;
from idle it bypasses settling, and during an active turn it attempts same-turn
steer. A non-steerable provider keeps the refs pending for the next safe
boundary rather than starting a parallel turn or forcing an interrupt.

Issue events route by exact Issue node ID. PR conversation, review, diff-comment,
review-thread, and synchronize events route only while GitHub's native
association resolves to the bound Issue. Braid does not select or manage the PR;
missing or ambiguous association fails closed. Raw branch `push` has no PR node
identity and is not a wake source.

## Persistence and Failure Boundaries

SQLite persists transport facts, not task meaning: provider addresses, object
refs, versions/digests, delivery GUIDs, pending/urgent state, mirror remote IDs,
and outbox state. GitHub remains canonical for remote objects; app-server
remains canonical for thread/turn state.

A Wrapper transport disconnect cannot be converted into Agent completion or
failure. Braid leaves the provider turn status unknown, does not start a
replacement thread, and does not retry Agent-owned GitHub side effects. Comment
create/update uncertainty is Braid-owned: once a remote ID is known it is used
directly; an uncertain create recovers only from one matching canonical comment
within the bounded evidence window, otherwise publication fails closed. Human
edits or deletion are lifecycle/conflict facts and are never silently undone.

Protocol-specific facts live in [`app-server-protocol.md`](app-server-protocol.md);
GitHub-specific facts live in
[`github-transport-contract.md`](github-transport-contract.md); operating
isolation lives in [`operator-runbook.md`](operator-runbook.md).
