# Changelog

All notable changes to Braid are recorded here. The project follows Semantic
Versioning once release artifacts are published.

## [0.2.0] - 2026-08-24

### Changed

- Collapsed all pre-release schema migrations into `migrations/0001_initial.sql`
  and reset the supported schema version to 1.
- Replaced shell `git` invocations in worktree handling with the `git2` crate.
- Replaced the hand-written `reqwest`/JWT GitHub App client in `src/github.rs`
  with `octocrab` for App authentication, installation tokens, and REST calls.
  GraphQL remains raw `octocrab` POSTs to keep the existing query envelope.
- Split `src/provider.rs` into `src/provider/mod.rs`, `src/provider/codex.rs`,
  `src/provider/pi.rs`, and `src/provider/util.rs`.
- Aligned `jsonwebtoken` with the version required by `octocrab`.

### Fixed

- Updated `scripts/tests/00_clean_install.sh`, `30_issue_agent.sh`, and
  `40_context_lifecycle.sh` for the collapsed schema version 1.

## [Unreleased]

### Changed

- Replaced the withdrawn Python turn-mirroring prototype with a Rust runtime
  foundation built around GitHub Context.

### Added

- Versioned TOML configuration and Agent Profile inspection.
- Forward-only SQLite schema migrations with checksums and backups.
- OTLP traces, logs, and metrics with parent-based trace sampling.
- Public operator diagnostics and portable macOS arm64 packaging.
- GitHub App installation authentication and bounded GraphQL pagination for
  canonical Issue, PR, Project V2, relationship, comment, review, and review
  thread reads.
- Deterministic agent-facing Markdown Context, HTML-comment filtering,
  Context Revision/pressure enforcement, and durable deletion tombstones.
- Public `github probe` and `context issue|pr` diagnostic commands.
- A bounded public Context page-size diagnostic for repeatable real GraphQL
  pagination campaigns without manufacturing high-volume comment fixtures.
- Verified Axum webhook ingress, durable delivery/event ledgers, canonical
  GitHub reconciliation, debounce/count scheduling, trusted visible mention
  resolution, Braid-owned `eyes` outbox, and owner fencing.
- Supervised free Quick Tunnel ingress with signed public readiness, App
  webhook handoff/restoration, delivery inspection/redelivery, and bounded
  transport status.
- The public `scripts/tests/20_ingress_scheduler.sh` real-object campaign for
  Slice 2 transport behavior, with provider turns intentionally disabled.
- Codex app-server NDJSON ownership, Profile materialization, complete Markdown
  Context injection, Issue Agent sessions/turns/steer, sampled provider
  evidence, and durable assignment/turn lifecycle state in schema version 4.
- Forward-only schema version 5 and desired-state Operational Status Comments
  for provider-outcome unknown, excluded from future GitHub Context by their
  persisted node IDs.
- Forward-only schema version 6 and durable Hard Invalidation records that
  fence active turns, replace stale Codex sessions with complete current Issue
  Context, and expose old/new sessions plus Context revisions through public
  operator status.
- The public `scripts/tests/40_context_lifecycle.sh` real GitHub/Quick
  Tunnel/Codex gate for idle and active Issue-description Hard Invalidation,
  stale-turn reaction fencing, current-Context continuation, minimized-comment
  reconciliation, unminimize Wake, and deletion tombstones. Root reconciliation
  now compares the human-visible Issue/PR projection so ordinary comment
  activity cannot masquerade as a description edit through GitHub `updatedAt`.
- Issue close now preserves an already-running turn, grants exactly one
  reaction-free Finalization Turn, and atomically sleeps the Agent Group.
  Closed activity cannot grant another turn; reopen rebuilds complete current
  GitHub Context in a fresh provider session and releases one ordinary
  debounced Wake. Public status reports bounded finalization evidence.
- Forward-only schema version 7 adds provider-session resume evidence,
  assignment-generation-scoped Operational Status ownership, and observable
  normal/soft/hard/unavailable Context pressure. Braid reconnects a lost
  app-server, resumes the same compatible physical thread, and keeps transport
  loss distinct from provider protocol failure.
- Forward-only schema version 8 adds public `braid gh` write receipts,
  concurrency claims, and comment-ID-keyed Implementation Request progress.
  The first write vertical mechanically attributes App-authored comments and
  converges concurrent `pr ensure` calls through a deterministic same-tree
  bootstrap branch, Draft PR, and native Issue association.
- `scripts/tests/50_issue_to_pr.sh` exercises that bounded first Slice 5
  vertical against real GitHub objects while leaving PR Agent/worktree/review
  behavior explicitly unproven.
- The first real Slice 5 campaign converged two concurrent ensure processes from
  Issue #33 and App comment `5294048165` to one durable receipt, one same-tree
  bootstrap branch, and Draft PR #34 with its native Issue association. Cleanup
  closed both Work Items and deleted the temporary branch.
- Forward-only schema version 9 records PR worktree source/head/branch identity.
  A successful `pr ensure` now activates the selected PR Profile immediately,
  materializes current Associated-Issue plus PR Context, provisions a dedicated
  generation-scoped worktree, and starts a distinct Codex session there.
- The expanded `scripts/tests/50_issue_to_pr.sh` campaign passed against Issue
  #43 and Draft PR #44. The PR Implementation Agent pushed one exact requested
  file from its runtime-owned worktree and published its own concise attributed
  PR comment; no turn mirror appeared. The campaign closed both Work Items,
  removed all five attempted fixture branches, and left review, cross-surface
  invalidation, direct-identity variants, and PR lifecycle as explicit gaps.
- Forward-only schema version 10 records visible content digests and one
  Issue-description cursor per native association. PR review-thread lifecycle
  can now replace stale PR Context in the existing worktree, while open
  Associated-Issue description edits create a PR-scoped invalidation only after
  the ordinary debounce/count boundary.
- The Slice 5 campaign now provisions a temporary free Quick Tunnel and
  repository webhook, exercises a real review comment/thread, and checks an
  active cross-surface replacement without restoring Braid turn mirroring.
  The schema-v10 candidate passed that expanded gate against closed Issue #57
  and PR #58 with one continuation, a preserved worktree, and no operator
  correction.
- `braid gh comment create` now normalizes an accidentally repeated generated
  attribution prefix to one quote block, and Agent system prompts state that
  callers pass only the message body to that command.
- Reconciliation now treats the first observation of a review thread as
  canonical state rather than a second Wake; only a known resolved thread
  becoming unresolved owns the thread-level Wake.
- Forward-only schema version 11 replaces per-association Issue-description
  digests with one Issue-owned visible-description comparison state. Agent
  Context, Event References, public Context diagnostics, and `braid gh` results
  no longer expose Context fingerprints, GraphQL node IDs, local receipt IDs,
  provider IDs, or other internal correlation values; `braid gh receipt` is
  removed in favor of retrying the semantic write operation itself.
- Review-thread webhook lifecycle is no longer ordered against the different
  GraphQL projection shape. Braid accepts `resolved`/`unresolved`, rereads the
  canonical thread, and normalizes that state without losing the Context
  replacement or generating a second semantic change.
- The schema-v11 Slice 5 campaign passed against closed Issue #62 and Draft PR
  #63: semantic `braid gh` results, concurrent idempotency, exact one-file diff,
  location-based resolved-thread Context, debounced Associated-Issue
  invalidation, one continuation, and worktree preservation all converged.
- The expanded Slice 4 public campaign now proves in-place provider resume plus
  a subsequent debounced Wake, complete soft-pressure Context delivery with one
  Operational Status Comment, and hard-pressure refusal with no provider
  session, turn, truncation, or generated summary.
- Desired-state trusted-mention reactions, including removal of stale active
  reactions after terminal convergence, and Agent-origin echo suppression by
  stable actor identity or public Profile attribution.
- `serve --transport-only` for the bounded transport campaign and the public
  `scripts/tests/30_issue_agent.sh` real GitHub/Quick Tunnel/Codex Issue Agent
  gate, covering debounce/count turns, edit steer, normal/failed/unknown
  outcomes, lifecycle reactions, Agent self-publication, and absence of Braid
  turn mirroring. The PoC uses trusted `@braid` to activate a dormant Issue
  because an ordinary GitHub App is not a standard assignable user.
- PR Work Items now share the explicit close/finalize/sleep/reopen lifecycle:
  reopen rebuilds current Context in a fresh provider session while preserving
  the dedicated worktree, and merge grants one final Finalization Turn before
  retiring the assignment, Agent, session, and worktree.
- Canonical Work Item state no longer lets the issue-shaped `closed` state on a
  merged PR conversation event downgrade the more precise `merged` root state;
  Finalization therefore observes the real terminal lifecycle.
- Public Agent Group status now includes the bounded logical turn count used by
  black-box campaigns to distinguish receipt, wake, and no-wake without relying
  on model prose or internal database inspection.
- Runtime supervision now treats unexpected worker exit as a whole-runtime
  readiness failure, handles SIGTERM and Ctrl-C, applies bounded worker/outbox/
  ingress/health/OTel shutdown, and preserves active turns as neutral `unknown`
  on compatible restart instead of creating a parallel turn.
- Unexpected runtime-owned Quick Tunnel exit now immediately marks bounded
  health unavailable and attempts to restore the prior GitHub App webhook while
  reconciliation remains available.
- Runtime-owned Quick Tunnel startup now treats the registered child as a
  candidate, verifies its public signed ingress before any App webhook mutation,
  and discards/retries bounded unreachable candidates instead of publishing a
  dead temporary URL.
- Runtime-owned uncertain comment creates reread bounded canonical comments and
  converge only one exact App-authored body candidate; multiple candidates are
  an explicit ambiguous write rather than a duplicate retry.
- `scripts/tests/60_operations.sh` composes clean packaging, schema-compatible
  rollback, owner fencing, graceful/forced process control, runtime-owned tunnel
  loss, and measured normal/incident OpenTelemetry sampling through public
  boundaries.
