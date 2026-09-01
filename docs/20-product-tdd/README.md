# Product TDD: GitHub Working Memory Runtime

This document owns Braid's cross-unit authority, topology, state ownership, and
Rust realization. It deliberately excludes unit algorithms that are evident
from typed code and external wire details owned by the linked contracts.

## Admission

- **Dependent units**: GitHub ingress/API/reconciliation, Context projector,
  event classifier and scheduler, Agent Group/session manager, Codex adapter,
  worktree provisioner, Braid GitHub writer, durable store/outbox,
  observability, tunnel supervisor, and CLI/runtime.
- **Failure if lost**: GitHub edits could leave stale provider context active,
  direct Agent writes could feed back indefinitely, Issue and PR profiles could
  share the wrong memory/worktree, or a process restart could duplicate turns
  and GitHub mutations.
- **Why code is insufficient**: the contract crosses GitHub, provider,
  filesystem, database, telemetry, and Human-visible lifecycle authorities.

## Authority and Topology

```text
GitHub webhook ──verify/durable ingest──┐
                                       ├─> canonical object ledger
GitHub GraphQL/REST reconciliation ────┘          │
                                                  v
                                        Context materializer
                                                  │
                                  canonical diff + Event References
                                                  │
                              event classifier / debounce scheduler
                                                  │
                              Agent Group + logical session manager
                                                  │
                                     provider adapter (Codex v1)

Agent ── braid gh ──> Braid write outbox ──> GitHub ──> origin-correlated echo
Agent ── ordinary gh/git ─────────────────> GitHub ──> ordinary external event

Quick Tunnel ──> loopback webhook ingress
OTel SDK ──sampled OTLP──> operator-selected endpoint
```

- **GitHub** owns current Issue/PR bodies, metadata, relationships, comments,
  reviews, lifecycle, identity, and App permissions.
- **GitHub Context** is a deterministic, complete projection of that state plus
  durable deletion tombstones. It is the collaboration memory authority.
- **Provider** owns physical session data, model execution, provider-native
  compaction, turn lifecycle, and resume availability.
- **Coding Agent** owns interpretation, design/implementation choices, code,
  verification, and its Git/GitHub operations.
- **Braid** owns canonical reads, projection, logical Agent Group/session
  generations, scheduling, Context fencing/replacement, provider addressing,
  PR worktree provisioning, Braid-authored GitHub writes, and mechanical
  idempotency/reconciliation.

SQLite stores enough canonical object metadata and lifecycle tombstones to
compare versions, but it does not become an alternate Issue/PR or provider
transcript authority. A full Context is rebuilt from GitHub before every turn.

## Single Source of Truth for Schema

A schema must have exactly one authority. When the same shape is defined both
by typed Rust structs and by a hand-written text template, the two drift and
Humans lose track of which one is "real". The Rust type is the source of truth;
any file that claims to be a valid instance of that schema must be provable
against it.

- **`Config` owns `braid.toml`**: `src/config.rs` defines the single source of
truth for the config schema. `braid setup` builds a `Config` value and
serializes it, rather than formatting a template. If the type changes, the
compiler forces the generator to change with it.
- **Round-trip tests for generated artifacts**: Any code that emits a
structured file a Human might edit must be covered by a test that parses the
output back through the canonical type and validates it.
- **No parallel string templates**: Do not maintain a separate text template
for a file whose shape is already defined by a Rust type. Avoid
`format!`-based generators for config, manifest, or protocol files.
- **ROI boundary**: Not every documentation snippet is a schema instance.
Only canonical or generated artifacts participate in SSoT validation. For
example, `config.example.toml` is the canonical starter template, so it is
loaded through `Config::load` in tests; a partial snippet in a runbook is not.
- **Migrations are the exception**: SQLite schema is forward-only, immutable,
and checksum-verified; it is not derived from Rust structs because the database
must outlive any single binary version.

## Rust Runtime Shape

MVP is one Rust package and one `braid` binary rather than a workspace of thin
crates. Modules are deep and align with authority boundaries:

| Module | Owned interface |
| --- | --- |
| `home` | User root resolution, instance registry, `~/.braid` layout, port allocation, and config-path precedence. |
| `config` | Versioned config/Profile loading, validation, effective defaults, and diagnostic projection. |
| `github::webhook` | Raw-body HMAC verification and typed open-enum webhook admission. |
| `github::client` | GitHub App auth, bounded typed GraphQL pagination, REST writes, reactions, and canonical rereads. |
| `context` | Canonical snapshot model, HTML-comment removal, deterministic Markdown rendering, budget, and revision. |
| `events` | Canonical diff classification and compact Event Reference rendering. |
| `store` | One dedicated SQLite actor, transactions, migrations, leases, ledgers, sessions, batches, and outbox. |
| `scheduler` | Quiet/count/urgent coalescing and single-flight group turn claims. |
| `producer` | Webhook/GraphQL ingress → canonical diff → classified events (`ingress`, `reconcile`). |
| `queue` | Per-work-item per-agent-group quiet window, batch emission, claim decisions, context-pressure policy, and store-side reset fencing (`scheduler`). Never touches provider sessions or connections. |
| `outbox` | Drain the GitHub write outbox (reactions, comments, statuses) with uncertain-write recovery. Leaf over `store` + `github`, called by ingress and the runtime drain loop. |
| `group` | Agent Group workers that own every provider connection epoch (connect, resume, drive, reconnect), the dispatch/materialization half that executes queue decisions against `AgentSession`s, provider supervision/prompts/attribution, and the per-epoch in-process `SessionManager` (`issue_agent`, `pr_agent`, `dispatch`, `provider`, `session_manager`). |
| `agent_session` | Core `AgentSession` trait and event stream (`TurnStarted`, `TurnTerminal`, `Failed`). Core callers operate sessions only through `send_user_msg`; the event stream is the single authority for lifecycle facts. |
| `provider::session` | `ProviderAgentSession` adapter that maps `AgentSession` to `AgentProvider` primitives and translates provider notifications into `SessionEvent`s, deduplicating the provider's response-side and notification-side observation of the same fact. |
| `session_manager` | In-process `SessionManager` keyed by provider thread id; start/resume/get. Ephemeral per connection epoch: it is rebuilt from the durable store on every (re)connect because sessions bind the epoch's provider handle. |
| `provider` | Provider-neutral capability contract and Codex NDJSON implementation. |
| `worktree` | Validate a Profile source checkout, resolve the bound ref (PR head, sole Development branch, or default origin branch), provision one generation-scoped worktree per Agent Group, and expose recovery diagnostics; no Git-operation sandbox. |
| `writer` | `braid gh`, attribution, reaction/status desired state, and write-outbox convergence. |
| `telemetry` | Trace/metric/log creation, payload events, sampling configuration, and OTLP export. |
| `tunnel` | Wrangler Quick Tunnel supervision and webhook URL handoff. |
| `runtime` | Owner lease, worker supervision, boot-time configuration gates, shutdown ordering, health, and public operator state. Never touches provider connections or sessions directly. |
| `cli` | `serve`, `config`, `doctor`, `profile`, `gh`, `status`, and migration/version surfaces. |

Module dependencies point one way only: `runtime` → `group` → `queue`, and
`runtime` → `producer` → `outbox`/`health`; `queue`, `outbox`, and `health`
sit above the leaf modules (`store`, `context`, `github`, `config`,
`provider`, `worktree`, `telemetry`) and no lower layer imports an upper one.

### Internal Event Model

`protocol.rs` owns the typed, platform-neutral event contract `EventKind`
(`assign`, `unassign`, `mention`, `wake`, `invalidate`, `lifecycle`,
`origin_echo`, `noop`). A producer translates platform deliveries into
`EventKind` at ingress and records only the internal kind plus the per-platform
opaque Event Reference; `queue` and `group` consume `EventKind` exclusively
and never branch on platform event names or actions. This is the seam at
which a future non-GitHub platform plugs in: it adds a producer mapping, not
new consumer logic. The current GitHub mapping is owned by
[`github.md`](github.md).

### State authority

Every piece of state has exactly one authority; everything else is a
best-effort one-way projection from it. Nonessential state is not persisted.

| State | Authority | Projections |
| --- | --- | --- |
| Work Items, assignments, turns, context ledger/resets, queue/batches, outbox, owner lease | Durable store (SQLite) | In-memory `RunningAgentTurn` (claim cache for the in-flight turn), health snapshot |
| GitHub canonical state | GitHub | `canonical_objects` / `sync_cursors` snapshots for diffing |
| Physical session identity (`provider_session_id`) | Durable store (`provider_sessions`) | `SessionManager` map key + adapter `thread_id` (both ephemeral, rebuilt per epoch) |
| Current provider turn | Provider process | `SessionEvent` stream (exactly one `TurnStarted`/`TurnTerminal` per turn; receiver handed off with the turn, never re-subscribed) → durable store; resume fencing as the cross-epoch backstop |
| Provider connectivity | Provider connection | `AgentProvider::closed()` future → worker epoch loop → health snapshot + blocked-session records |
| Worktree presence | Filesystem + git | `worktrees` table (refreshed by inspection at prepare time) |

Selected dependency baseline, verified against crates.io on 2026-08-13:

- Tokio 1.53 for async runtime/process/I/O/signal ownership;
- Axum 0.8 for loopback HTTP and Reqwest 0.13 with rustls for GitHub/OTLP HTTP;
- Serde 1.0 and serde_json 1.0 for external JSON boundaries;
- rusqlite 0.40 with bundled SQLite, used only by a dedicated blocking DB
  actor so network awaits never occur inside transactions;
- Clap 4.6 (with `env` feature), `serde_path_to_error` for TOML error paths,
  `thiserror` for boundary errors, and `anyhow` only at CLI/runtime
  composition boundaries;
- HMAC 0.13/SHA-2 0.11 for webhook verification and context revisions,
  `jsonwebtoken` 11 for GitHub App JWTs;
- Comrak 0.54 to identify actual Markdown HTML comment nodes while preserving
  code literals;
- OpenTelemetry/SDK/OTLP 0.32 plus tracing-opentelemetry 0.33;
- UUID 1.24, `time` 0.3, and `url` for typed identities and timestamps.

Versions become lower/upper compatible ranges in `Cargo.toml`; `Cargo.lock` is
committed for the released binary. Rust 1.93 is the first verified toolchain.

## Cross-Unit Invariants

1. Every provider turn belongs to exactly one Work Item, Profile,
   Assignment Generation, Context Revision, and physical Provider Session.
2. A group admits one active turn in MVP. A stale Context Revision cannot
   publish Braid-owned writes after fencing.
3. Every turn uses a freshly materialized complete Context. Required pagination
   or hard-budget failure blocks before provider input.
4. A Provider Session is reusable only while Context schema/revision and
   effective instruction revision remain compatible. Hard Invalidation creates
   a fresh physical session in Codex v1.
5. Event References contain no copied GitHub prose. They identify what changed
   and where; current Context supplies durable content and `gh` supplies detail.
6. Braid App writes carrying durable operation correlation and direct writes
   from the Profile's explicitly configured stable GitHub actor are Agent-origin
   for self-wake/reset suppression. Other direct `gh` writes remain allowed but
   are processed as external activity.
7. Braid never mirrors turn output. Its only unsolicited comments are mutable
   Operational Status Comments.
8. Ordinary batches have no active/terminal reactions. Those states apply only
   to the exact trusted `@braid` comment that started the turn.
9. GitHub/network I/O never occurs in a SQLite transaction. Every Braid-owned
   mutation has a durable intent before leaving the process.
10. Provider terminal means only that a turn ended; it never proves task,
    implementation, review, or acceptance success.

## Durable State and Migrations

SQLite starts in WAL mode with foreign keys, a busy timeout, and FULL
synchronous durability. One DB actor serializes writes. Schema v1 owns:

- `schema_migrations(version, name, checksum, applied_at)`;
- `owner_leases(scope, generation, owner_id, expires_at)`;
- `repositories` and `work_items` keyed by stable GitHub node IDs;
- `profiles` with immutable revision and effective-config digest;
- `assignments` and `agent_instances` with group/profile/generation/lifecycle;
- `provider_sessions` and `turns` with opaque provider IDs and Context revision;
- `worktrees` for the default PR Implementation Agent workspace;
- `associations` for direct Issue↔PR edges and their observed version;
- `issue_context_sources` for one exact visible-description comparison state
  per Issue, independent of association fan-out;
- `canonical_objects` for latest object version/lifecycle, the internal
  reconciliation equality fingerprint, and deletion tombstones;
- `deliveries` and `events` for webhook/reconciliation dedupe/classification;
- `scheduler_batches` and `batch_events` for quiet/count/urgent state;
- `write_intents`, `reaction_targets`, and `status_comments` for Braid-owned
  GitHub convergence;
- `sync_cursors` for repository reconciliation.

Migration files are embedded, monotonically numbered, forward-only, and never
edited after release. Startup takes an exclusive migration lease, verifies all
previous checksums, applies each migration in one transaction, and rejects a DB
newer than the binary. Compatible application rollback is declared per release;
an incompatible schema rollback restores the pre-migration backup rather than
running a down migration.

For any Agent-serving Profile, `workspace` names a clean source Git checkout of
the configured repository, not the directory in which the Agent edits. Every
Agent Group session runs in a dedicated generation-scoped worktree that Braid
provisions from that checkout:

- PR Agent Group: `runtime.root/worktrees/pr-<number>/<profile>-g<generation>`,
  bound to the fetched PR head;
- Issue Agent Group: `runtime.root/worktrees/issue-<number>/<profile>-g<generation>`,
  bound to the Issue's sole same-repository Development linked branch when
  exactly one exists, otherwise to the repository default branch
  (`refs/remotes/origin/<default>`). Several Development branches are ambiguous
  and block materialization with an operator diagnostic, mirroring `pr ensure`
  disambiguation.

SQLite records the resolved source, worktree, bound ref, and local branch as
operational facts. The provider session is started and later resumed only
against that worktree. This provides isolation and recovery identity without
turning Braid into a Git policy engine.

## Error and Concurrency Model

External shapes deserialize into typed structures with explicit unknown
variants; unknown methods/actions/unions become bounded `unsupported` evidence
rather than generic JSON forwarded to the Agent. Boundary errors carry a stable
category plus source error; Human-facing status never exposes Rust backtraces as
Agent prose.

The runtime has one repository owner lease and per-Agent-Group single-flight
turn ownership. Lease generation fences stopped processes. Braid-owned creates
and updates use the write-outbox state machine; direct Agent `gh`/`git` actions
are outside it and converge only when GitHub reports them.

## Realization Pointers

- Exact Context and revision: [`context.md`](context.md)
- Event, group, session, and reaction state machines: [`lifecycle.md`](lifecycle.md)
- Codex mapping and provider seam: [`app-server.md`](app-server.md)
- GitHub ingress, canonical reads, associations, identity, and writes:
  [`github.md`](github.md)
- Packaging, OTel, tunnel, migration, and operation:
  [`../40-deployment/README.md`](../40-deployment/README.md)
- External acceptance oracle: [`../10-prd/acceptance.md`](../10-prd/acceptance.md)
