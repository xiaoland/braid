# Product TDD: Braid Transport Contract

This document is the cross-unit contract between GitHub, Braid, the Coding
Agent, and Codex app-server. It owns authority, topology, and compatibility
boundaries that cannot be recovered safely from one unit's implementation.

## Admission

- **Dependent units**: GitHub ingress/API and reconciliation, durable store and
  scheduler, provider adapter, turn controller, mirror projection/publisher,
  and the operator runtime.
- **Failure if lost**: a unit could interpret collaboration state, provider
  context, or remote publication as its own authority, causing duplicate turns,
  stale work, or irreversible GitHub side effects.
- **Why code is insufficient**: the contract spans independent authorities and
  external protocols; a local implementation cannot make the ownership split
  obvious to every participating unit.

## Authority and Topology

```text
GitHub webhook -> temporary Tunnel -> loopback ingress
  -> durable event inbox -> quiet/urgent scheduler
  -> one bound Codex app-server thread

GitHub GraphQL reconciliation -> same event inbox
app-server turn items -> bounded projection -> comment outbox
  -> one canonical GitHub comment + link-only FYIs
```

- **GitHub** owns repository, Issue, PR, comment, review, review-thread,
  association, identity, and permission state.
- **Codex app-server** owns provider thread data, compaction, active-turn
  lifecycle, execution, and resume semantics.
- **Coding Agent** owns interpretation, readiness, plans, code, verification,
  and authorized Git/GitHub actions.
- **Braid** owns one Issue-to-opaque-thread binding, canonical event delivery,
  mechanical scheduling, turn projection, comment publication, and transport
  idempotency state. It stores no provider transcript or semantic task state.

## Cross-Unit Contract

- One active binding admits at most one Agent turn. Ordinary canonical events
  reset a quiet deadline; a trusted exact visible `@agent` hint bypasses that
  delay or attempts same-turn steering. A non-steerable provider leaves refs
  pending for the next safe boundary rather than starting a parallel turn.
- Issue events route by exact Issue node ID. Pull-request conversation, review,
  diff-comment, review-thread, and synchronize events route only while GitHub's
  native association resolves to the bound Issue. Missing or ambiguous
  association fails closed; raw repository push is not a wake source.
- Wrapper-origin provider context contains only event/action/actor/object and
  surface references plus digests. GitHub-authored prose remains canonical
  state that the Agent fetches through `gh`.
- A turn freezes one canonical response surface. Other participating surfaces
  receive at most one short FYI link and never a copied projection.
- SQLite persists transport facts—provider addresses, object refs,
  versions/digests, delivery GUIDs, pending/urgent state, mirror remote IDs,
  outbox state, and sync cursors—not task meaning.
- A transport disconnect never becomes Agent completion or failure and never
  creates a replacement thread. Once a remote comment ID is known, updates use
  it directly; uncertain creation recovers only from one matching canonical
  comment and otherwise fails closed. Human edits and deletion remain conflict
  or lifecycle facts and are never silently undone.

## Realization Pointers

- Provider wire and lifecycle: [`app-server.md`](app-server.md).
- GitHub ingress, identity, reconciliation, and native association:
  [`github.md`](github.md).
- Turn item reduction, Markdown publication, bounds, and recovery:
  [`turn-projection.md`](turn-projection.md).
- Isolation, preflight, handoff, rollback, and runtime operations:
  [`../40-deployment/README.md`](../40-deployment/README.md).

