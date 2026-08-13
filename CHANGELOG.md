# Changelog

All notable changes to Braid are recorded here. The project follows Semantic
Versioning once release artifacts are published.

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
- Desired-state trusted-mention reactions, including removal of stale active
  reactions after terminal convergence, and Agent-origin echo suppression by
  stable actor identity or public Profile attribution.
- `serve --transport-only` for the bounded transport campaign and the public
  `scripts/tests/30_issue_agent.sh` real GitHub/Quick Tunnel/Codex Issue Agent
  gate. The PoC uses trusted `@braid` to activate a dormant Issue because an
  ordinary GitHub App is not a standard assignable user.
