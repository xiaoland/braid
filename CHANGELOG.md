# Changelog

All notable changes to Braid are recorded here. The project follows Semantic
Versioning once release artifacts are published.

## [0.3.1] - unreleased

### Fixed

- `braid setup` pinned a hardcoded profile `adapter_version`, so config
  validation rejected the generated config whenever the discovered runtime
  version differed (e.g. codex-cli 0.151.0). The profile now pins the
  discovered runtime version.
- Reopen reactivation was not idempotent: when a newer assignment generation
  was already active (or the reopen was delivered twice), reactivation
  selected a stale sleeping generation, hit the unique active-assignment
  index, and error-looped every tick, permanently wedging the group scheduler.
  Reactivation is now an ensure-style no-op when the group is already
  materializing/active/finalizing.
- A trusted `@braid` mention on a closed Work Item activated a new assignment
  generation. Activation (`assign`/`mention`) now applies only to open Work
  Items; closed groups sleep until reopen, as the lifecycle contract states.

### Changed

- The event ledger now stores the typed, platform-neutral `EventKind`
  (`assign`/`unassign`/`mention`/`wake`/`invalidate`/`lifecycle`/
  `origin_echo`/`noop`) plus a semantic detail instead of ad-hoc
  GitHub-shaped classification strings (schema v2). Producers map platform
  deliveries at ingress; queue and group consumers branch on `EventKind`
  only. Cross-surface invalidation folds into `invalidate` with
  `detail='cross_surface'`; agent-origin echoes and ping/unknown deliveries
  are evidence-only and consumed at ingest.
- `braid setup` now creates the Profile workspace directory it writes into
  the config; previously a fresh setup left the workspace missing and the
  first Agent turn materialization failed with the Assignment parked in
  `blocked`.
- `braid setup` now bootstraps the instance-scoped Codex provider home:
  it imports `~/.codex/auth.json` when present and otherwise prints explicit
  `CODEX_HOME=... codex login` instructions. Previously `braid serve` ran
  with the provider perpetually disconnected and no guidance.
- `braid doctor` gained a "Codex credentials" check for provider-home
  authentication, so the gap is caught before serving.

### Added

- Setup output and the user manual now state the Issue activation contract:
  a trusted `@braid` mention from a MAINTAIN/ADMIN actor. Native Issue
  assignment is GitHub-side Agent App provisioning that ordinary
  manifest-created Apps cannot obtain; Braid detects the mode at runtime.

- The GitHub installation client no longer pins the initial installation
  token: it is built via octocrab's installation auth state, which caches and
  auto-refreshes the token. Previously every API call began failing with 401
  "Bad credentials" one hour after `serve` started (token expiry), silently
  wedging mention resolution, reactions, and the write outbox until restart.
- Mention-authority resolution now backs off exponentially (2s to 60s) on
  persistent GitHub errors instead of retrying every 250ms scheduler tick.

- PR worktree provisioning fetched through libgit2, which ignores the
  operator's credential helpers and proxy configuration and failed on real
  networks ("no TLS stream available"). The fetch now uses the configured
  system `git` executable; libgit2 remains for local reference/worktree
  operations.


## [0.3.0] - 2026-08-31

### Added

- User/instance namespace: one user root (`~/.braid` or `BRAID_USER_HOME`)
  holds a `registry.toml`, optional user defaults, shared provider secrets, and
  per-instance directories under `instances/<key>/`. This replaces the flat
  per-owner files introduced in 0.2.3.
- `src/home.rs` resolves the user root, loads the instance registry,
  validates instance keys, allocates free loopback ingress/health port pairs,
  and implements the config-path precedence chain.
- `scripts/tests/05_instances.sh` exercises `--config`/`--instance`/
  `BRAID_INSTANCE`/`BRAID_INSTANCE_HOME` resolution and the doctor's
  cross-instance port-conflict check.
- Config schema version 2 with `[[runtimes]]`, `[[llm_providers]]`, and profile
  `adapter_type` / `adapter_version` references. Runtime connectivity no longer
  lives in profiles.
- `src/agent_session.rs` defines the core `AgentSession` trait and event stream;
  `ProviderAgentSession` in `src/provider/session.rs` maps it to the existing
  provider primitives and owns physical session creation/replacement/resume.
- `SessionManager` in `src/group/session_manager.rs` manages per-provider-thread
  `ProviderAgentSession` handles (start/resume/get), keyed by the thread id the
  adapter actually created. It is ephemeral per connection epoch and rebuilt
  from the durable store on reconnect.
- New architecture boundary modules `src/producer/`, `src/queue/`, and
  `src/group/` own the implementation modules (`ingress`/`reconcile`,
  `scheduler`/`outbox`, `issue_agent`/`pr_agent`/`provider`/`session_manager`);
  `src/runtime/mod.rs` is the coordinator (owner lease, worker supervision,
  shutdown ordering, health).

### Changed

- **Breaking**: `schema_version` must be `2`; old v1 configs must be regenerated
  by re-running `braid setup --instance <key>`.
- **Breaking**: `--worker` and `--home` are removed. Config-loading commands
  now take `--config <PATH>` or `--instance <KEY>` (or `BRAID_INSTANCE`).
  `braid setup` takes `--user-home <DIR>` and `--instance <KEY>`.
- **Breaking**: Secrets are split. The instance `secrets.toml` holds only the
  webhook secret; provider API keys live in `~/.braid/secrets/<provider>.toml`
  and are referenced by path.
- **Breaking**: The runtime default root is now `<config_dir>/state` and the
  default database file is `state/braid.sqlite3`.
- **Breaking**: Config schema v2 now requires an `[instance]` section with a
  `key` and is the only supported config schema.
- `provider.codex` / `provider.pi` are replaced by `[[runtimes]]` entries.
- `braid setup` discovers local runtimes, prints install instructions when none
  are found, and never auto-installs. Manual flags `--runtime-executable` and
  `--runtime-api-url` bypass discovery.
- `braid doctor` loads the registry and reports duplicate or colliding
  ingress/health ports across registered instances.
- Telemetry now tags `service.instance.id` from the config instance key.
- `braid status` prints the instance key.
- Config TOML parse errors now include the dotted path to the offending key
  via `serde_path_to_error`.
- Clap `env` integration binds `BRAID_INSTANCE`, `BRAID_USER_HOME`, and the
  setup `--instance` flag to their environment variables.
- All scheduler dispatch paths (`start_next_agent_turn`, `forward_urgent_steer`,
  `begin_active_context_reset`, `materialize_context_reset`) and all
  assignment/reactivation/resume materialization paths operate sessions only
  through `AgentSession`/`SessionManager`; no core code calls
  `provider.start_session`/`inject_context`/`resume_session` directly.
- Agent group loops (`issue_agent_worker`, `pr_agent_worker`) consume
  `SessionEvent` from the active `AgentSession` rather than raw provider
  notifications.
- `connect_provider` returns `Arc<dyn AgentProvider>` so the adapter and
  `SessionManager` share ownership.
- `SendResult::Started` no longer carries the provider turn id;
  `SessionEvent::TurnStarted` is the single authority for turn identity.
- `AgentSession::interrupt()` restores physical turn termination as a
  first-class contract operation — the control-plane sibling of steering
  (immediate, addressed to the observed in-flight turn, carrying control
  rather than input). `begin_active_context_reset` now fences the active turn
  in the store AND best-effort interrupts it (Codex `turn/interrupt`, Pi
  `abort`), matching the Hard Invalidation Sequence; the fence remains the
  correctness mechanism if the interrupt fails. The function moved to
  `group::dispatch` since it now touches sessions.
- The event protocol is now exactly-once: `TurnTerminal` carries the
  provider's error reason and `SessionEvent::Failed` is removed, so a turn
  has exactly one `TurnStarted` and exactly one `TurnTerminal` (synthesized
  as `Unknown` on session death). Connection death is connection-scoped and
  observed through the new `AgentProvider::closed()` future. The dispatcher's
  event receiver is handed to the drive loop inside `RunningAgentTurn`, so
  lifecycle facts cannot be lost to a subscription-timing gap.
- Provider connection epochs are now owned entirely by the group workers:
  `serve` validates the provider configuration at boot (fail-fast on
  misconfiguration) and each worker creates every connection — including the
  first — and retries transient connection failures. `serve` no longer exits
  when the provider is unreachable at boot; the worker reports the provider
  unavailable and keeps retrying, matching the reconnect semantics.
- The module graph is now acyclic with one-way layering (`runtime` → `group`
  → `queue`; `runtime` → `producer` → `outbox`/`health`). Dispatch and
  materialization moved from `queue::scheduler` to `group::dispatch` (they
  execute queue decisions against sessions, a group concern); the in-memory
  `RunningAgentTurn` claim cache moved from `producer::reconcile` to `queue`;
  `HealthSnapshot` moved to a new leaf `health` module; `LEASE_TTL_SECONDS`
  moved to `producer` (lease protocol home); `agent_attributions` moved to
  `config` (pure profile derivation); and the GitHub write outbox moved from
  `queue::outbox` to a leaf `outbox` module (it only needs `store` +
  `github`).

### Removed

- Deprecated `Config::provider_config()` bridge removed; callers use
  `default_provider_config()` or `RuntimeEntry` directly.
- Direct provider notification subscription removed from worker loops; the
  `AgentSession` event stream is the sole signal boundary.
- Dead `handle_provider_notification` fallback removed; provider `Activity`
  notifications are traced inside `ProviderAgentSession`.
- Unused `ProviderSession.model` field removed.
- Dead in-place reset surface removed: `send_user_msg`'s `reset_context_to`
  parameter, the adapter's `pending_reset`/`last_context_hash` bookkeeping,
  `SessionEvent::SessionReplaced`, `SessionManager::replace`, and
  `Store::replace_provider_session` were unreachable from every production
  path. Context replacement is orchestrated by the core through
  `SessionManager::start` with materialized context, matching the tested
  reset state machine.
- `SessionEvent::Failed` removed; connection death is connection-scoped and
  observed via `AgentProvider::closed()`.
### Fixed

- The adapter no longer emits two `TurnStarted` events per turn (the
  `turn/start` response and the async notification are two observations of
  one fact and are now deduplicated).
- A turn terminal emitted between dispatch and the drive loop's
  re-subscription was silently lost, leaving the store turn stuck in
  `running`; the dispatcher's receiver is now handed off inside
  `RunningAgentTurn`, so the consumer never re-subscribes mid-turn.
- An idle provider disconnect was never observed (the drive loop only
  listened while a turn was active), so every later dispatch failed into
  `unknown` forever without a reconnect; the worker now awaits
  `AgentProvider::closed()` regardless of turn state.
- A provider turn failure produced two terminal-ish signals (`TurnTerminal`
  and `Failed`); there is now exactly one `TurnTerminal` per started turn.
- The adapter now enforces the exactly-once terminal contract at its own
  boundary: `TurnCompleted` is accepted only for the currently tracked turn,
  so a duplicate or stale terminal (e.g. replayed after resume) is logged and
  ignored instead of double-emitting or — worse — clearing the tracking of a
  different in-flight turn and allowing a concurrent turn to be dispatched. A
  `TurnStarted` re-emitted after its terminal is likewise ignored.
- `SessionManager` is now scoped to one worker and one connection epoch.
  Previously it survived reconnects while its sessions kept the dead
  epoch's provider handle, so `resume` cache hits returned sessions that
  could never send again (including sessions marked failed on disconnect).

- Assignment/reactivation/reset materialization no longer creates two physical
  provider sessions (one raw, one adapter-owned) with the durable store
  recording the orphaned thread; the adapter-created thread is now the single
  recorded session, and turn history stays attached to the recorded thread
  across restarts.

## [0.2.3] - 2026-08-24

### Added

- `braid setup` now writes per-owner config (`~/.braid/braid-of-<owner>.toml`)
  and a single per-owner secrets file (`~/.braid/braid-of-<owner>.secrets.toml`).
- Runtime, `braid doctor`, tunnel verification, and the Pi provider now load
  secrets from the config file instead of requiring environment variables.
- `braid setup` now generates a composite GitHub App logo
  (`~/.braid/braid-of-<owner>-logo.png`) using a transparent Braid logo as the
  main content and the owner avatar as a bottom-right circular badge.
- Added `docs/user-manual/tunnel.md` explaining that `serve --tunnel` uses a
  free Cloudflare Quick Tunnel, automatically obtains a public URL, and updates
  the GitHub App webhook.

### Changed

- `braid setup` now generates `braid.toml` from the canonical `Config` type so
  the file is always valid against the current schema.

## [0.2.2] - 2026-08-24

### Changed

- Split `src/cli.rs` into a module tree (`cli/mod.rs`, `cli/config.rs`,
  `cli/profile.rs`, `cli/context_cmd.rs`, `cli/gh.rs`, `cli/gh_cmd.rs`,
  `cli/migrate.rs`, `cli/doctor_cmd.rs`, `cli/status.rs`, `cli/telemetry_cmd.rs`,
  `cli/helpers.rs`). Public entry point `cli::run` and all clap derive structs
  remain unchanged.
- Split `src/writer.rs` into a module tree (`writer/mod.rs`, `writer/prepare.rs`,
  `writer/ensure.rs`, `writer/comment.rs`, `writer/helpers.rs`). Public
  `create_comment` and `ensure_pull_request` signatures remain unchanged.

## [0.2.1] - 2026-08-24

### Changed

- Split `src/runtime.rs` into a module tree (`runtime/mod.rs`, `runtime/ingress.rs`,
  `runtime/outbox.rs`, `runtime/reconcile.rs`, `runtime/scheduler.rs`,
  `runtime/issue_agent.rs`, `runtime/pr_agent.rs`, `runtime/provider.rs`,
  `runtime/tunnel.rs`). Public entry point `runtime::serve` and health snapshot
  types remain unchanged.

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
