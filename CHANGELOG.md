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
