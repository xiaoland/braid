# Rust MVP Implementation

- **Goal**: Bring PR #94 — *Rebuild Braid around GitHub working memory* — to a mergeable state. The Rust runtime, GitHub Context, lifecycle, PR Agent worktree, `braid gh`, migrations, packaging, and operational convergence are implemented through schema v11. Acceptance pivoted from Codex to Pi because the Codex account hit its usage limit. Pi now works as a real provider via its `bash` tool, and the core Issue Agent journeys have converged. The remaining work is to make one clean full Slice 3 run (and then Slices 4–6) against real GitHub + Pi + the packaged binary, accounting for flaky GitHub API/network behavior.
- **Objective**: Replace the withdrawn Python turn-mirror prototype with the packaged Rust MVP specified by the Product Truth, Product TDD, provider/GitHub contracts, deployment contract, and real black-box acceptance oracle.
- **Guardrails**: GitHub Context is canonical working memory; do not preserve Python mirror/thread assumptions or add compatibility aliases. Do not constrain Agent `gh`/`git` beyond GitHub/provider/Profile permissions. Treat digest/hash as an exceptional internal mechanism, not a product or domain primitive: first repair the owning identity, version, lifecycle, or authority contract at the preceding architecture layer. Do not place hashes, UUIDs, delivery IDs, provider/session/turn/item IDs, or SQLite IDs in Agent Context; unavoidable internal identifiers remain transport-local and semantically inert. No GitHub/network await inside SQLite transactions. No internal fake/unit/component test suite; public diagnostic and campaign helpers live under `scripts/tests/`. Sampled telemetry intentionally preserves full evidence and is treated as sensitive. Keep this as the only active task packet and delete it after the three clean acceptance campaigns and durable release evidence close the task.
- **Verification**: `cargo fmt --check`, `cargo check --locked --all-targets`, and strict Clippy diagnose every slice. Each slice also has its public-boundary gate below. Product acceptance requires the clean packaged macOS arm64 artifact to pass every journey in `docs/10-prd/acceptance.md` three consecutive times without corrective operator action; internal state/logs never substitute. Linux x86_64 packaging is the immediate follow-on gate, not a blocker for the first macOS MVP release.
- **Handoff Baseline (2026-08-19)**: The working tree is clean at commit `8504cd6` (`feat(runtime): converge PR lifecycle and operations`), plus one task-packet commit on `feat/docs-namespace-migration`. `cargo fmt --check`, `cargo check --locked --all-targets`, and `cargo clippy --locked --all-targets` all pass. The previous agent's in-progress uncertain-write test harness (modified `scripts/tests/60_operations.sh`, `scripts/tests/README.md`, `src/github.rs`, and new `scripts/tests/connect_delay_proxy.py`) was moved to a WIP branch `handoff/slice7-wip` so the baseline branch remains known-good. The questionable `GitHubError::Response` → `is_unavailable()` change was reverted with that WIP; `Response` means a deterministic response-shape error and must not be treated as an uncertain transport outcome.
- **Realignment**: The active branch is `feat/rust-working-memory`, tracking PR #94. The old PR #2 was closed as superseded. The uncertain-write proxy harnesses and `60_operations` fragments stay on local `handoff/slice7-wip` and are not blocking baseline. Slice 7/acceptance remains the remaining gate for the PR.
- **Current Truth**: Product, Context, lifecycle, provider, GitHub, SQLite/OTel/deployment, and acceptance designs are closed in their durable owners. Slices 0–5 are implemented and previously had bounded real public-boundary evidence with Codex. Schema v11 carries Issue and PR lifecycle uniformly. With Pi, the provider seam works and the Issue Agent can use `gh`/`braid gh` through Pi's `bash` tool to publish real Agent comments. Slice 6 convergence implementations are present but some external-fault fixtures remain unproven.
- **Current Slice Evidence**: The expanded Slice 4 campaign previously passed against closed Issue #28 with Codex. Slice 3 evidence with Pi is accumulating below.
- **Latest Evidence**: With Pi, Braid accepts trusted `@braid` mentions, applies `eyes`/`rocket`/`+1`, and the Agent publishes attributed comments via `braid gh`. Ordinary debounce and count-threshold turns have converged. Recent failures are external GitHub API/network flakiness and the provider-disconnect fixture.
- **Slice 5 Evidence**: The final schema-v11 `scripts/tests/50_issue_to_pr.sh` campaign previously passed with Codex. It has not been rerun with Pi yet.
- **Slice 6 Evidence**: `scripts/tests/60_operations.sh` previously passed with Codex. It has not been rerun with Pi yet.
- **Environment Setup (2026-08-24)**:
  - GitHub App `braid-by-xiaoland`: App ID `4558000`, installation `153412294`, permissions `contents=write`, `issues=write`, `metadata=read`, `pull_requests=write`; required webhook events enabled.
  - App private key at `/Users/lanzhijiang/.braid/braid-by-xiaoland.pem` (mode `0600`).
  - Webhook secret exported as `BRAID_WEBHOOK_SECRET`.
  - Pi config `/Users/lanzhijiang/.braid/braid-pi.toml` uses `[provider.pi]` with `deepseek-chat`.
  - `config check`, `doctor` (except OTLP), `github probe`, `migrate apply` pass for the Pi config.
  - Packaged release artifact built and extracted: `/Users/lanzhijiang/.braid/install/braid-v0.1.0-aarch64-apple-darwin/bin/braid`.
  - Fixed stale schema assertions in `scripts/tests/30_issue_agent.sh` and `scripts/tests/40_context_lifecycle.sh` (10 → 11).
  - Cleaned up all prior Slice 2/3 fixture issues and stale webhooks.
- **Tunnel workaround**: Cloudflare Quick Tunnel public routes intermittently return Cloudflare error 1033. To unblock acceptance, installed `localtunnel` and patched scripts 20/30/40/50 to accept `BRAID_TEST_PUBLIC_WEBHOOK_URL` with any `https://*/webhook` host. A local HMAC-signing relay restores the `X-Hub-Signature-256` header because localtunnel strips it. A fresh localtunnel is started per run.
- **Campaign Progress**:
  - Slice 0 clean-install ✅ passed.
  - Slice 2 transport/scheduler ✅ passed via localtunnel repository-webhook mode.
  - Slice 3 Issue Agent (Pi): **core journeys converge**. The Agent reads the Issue, sleeps, publishes an attributed comment via `braid gh comment create`, and Braid applies `eyes`/`rocket`/`+1` correctly. Ordinary debounce and count-threshold also converge. Recent runs failed on external GitHub API flakiness (`EOF`, `TLS handshake timeout`) and the provider-disconnect fixture (killing the Pi shell wrapper leaves the Node child alive).
  - Slices 4–6 not yet rerun with Pi.
- **Provider pivot findings**:
  - Localtunnel strips GitHub's `X-Hub-Signature-256`; a local HMAC-signing relay restores signed webhook delivery to Braid.
  - Pi `--mode rpc` has a working `bash` tool; the Agent can call `gh` and `braid gh` directly.
  - Pi `--mode rpc` with `--no-session` does not return `sessionFile`; `--session-dir <workspace>/.braid/pi-sessions` fixes provider session materialization.
  - Pi does not emit a reliable `turn_end` per Braid turn; it emits `turn_end` after each assistant response step and `agent_settled` only when the session settles. Braid now treats `agent_settled` or `message_end` with `stopReason == "stop"` as terminal.
  - `github_actor_node_id` must be the App actor node id (`BOT_kgDOEtKIgA`), not the human user, for human `@braid` mentions to be trusted external activations.
  - `PiProvider` passes the current `braid` binary directory in `PATH` and `BRAID_CONFIG` to the Pi process so `braid gh` commands work.
  - The Slice 3 script must count Agent comments by `app_actor` (the GitHub App) because `braid gh` publishes as the App.
- **Code cleanup done**:
  - Renamed branch to `feat/rust-working-memory`; updated PR #94 head.
  - Closed/superseded PR #2.
  - Refactored provider into `AgentProvider` trait with `CodexProvider` and `PiProvider`.
  - Fixed Pi environment, notification thread/turn IDs, and terminal event handling.
  - Updated `scripts/tests/30_issue_agent.sh` for Pi (App actor, count-throttle delay, recursive provider kill).
  - Satisfied `cargo fmt --check`, `cargo check --locked --all-targets`, and `cargo clippy --locked --all-targets`.
- **Next Step**:
  1. Fix the provider-disconnect fixture so it reliably terminates the Pi Node child process.
  2. Run one final clean Slice 3 campaign; if it passes, proceed to Slices 4–6.
  3. On three clean full runs: update `CHANGELOG.md`, tag/release the macOS arm64 artifact, open the Linux x86_64 follow-on task, delete this packet.

## Impact Handshake

### Address and State Diff

- Remove `pyproject.toml`, `pdm.lock`, `.pdm-python`, `.pdm-build/`, `config.example.json`, and every Python file under `src/braid/`.
- Add one Rust package: `Cargo.toml`, committed `Cargo.lock`, `rust-toolchain.toml`, `rustfmt.toml`, `src/main.rs`, and deep modules named in `docs/20-product-tdd/README.md`.
- Add `CHANGELOG.md`, `LICENSE` if absent, versioned `config.example.toml`, embedded `migrations/`, and release/build metadata.
- Retain and update `README.md`, `AGENTS.md`, `glossary.md`, `docs/`, `docs/assets/`, and `scripts/tests/` as canonical/public surfaces.
- The CLI command remains `braid`; Python module imports, PDM commands, config JSON, and Python SQLite schema are intentionally unsupported.

### Data and Runtime Compatibility

- Rust DB schema begins at version 1 in a new configured database. It does not import the obsolete prototype DB. Startup detects a non-Braid-Rust or unsupported schema and refuses with an operator instruction to move it aside; it never mutates it speculatively.
- GitHub objects are reused only through canonical reread. No old mirror comment IDs or provider binding is imported; the withdrawn Braid instance must be stopped before the Rust App/webhook begins.
- The GitHub App may be reused after its permissions/subscriptions satisfy the new contract. Existing App-authored comments remain ordinary canonical comments unless explicitly recorded as Operational Status during the new generation.
- Codex 0.147/schema digests remain the first provider pin; no Python adapter code is copied.

### Blast Radius and Rollback

- Source/build/config/runtime change is intentionally repository-wide. Product docs and branding stay stable.
- Git history is the rollback for source. Runtime rollback to Python is not promised because it implements a withdrawn product. Before every DB migration, the stopped DB is backed up; binary rollback follows the declared schema compatibility matrix.
- External GitHub mutations begin only in the slice whose gate requires them and use the dedicated acceptance App/repository fixture.

## Linear Slices

The following slices are already implemented and are kept here as a reference for acceptance troubleshooting. The active work is now the PR-level gate described in **Next Step**.

### Slice 0 — Rust, Configuration, Storage, Telemetry, and Distribution Foundation

Implement the replacement and dependency baseline exactly as Product TDD specifies. The first binary exposes:

- `braid --version`;
- `braid config check --config ...`;
- `braid profile inspect --config ... --profile ...`;
- `braid migrate plan|apply --config ...`;
- `braid status --config ...` for stopped/local state;
- `braid doctor --config ...` with filesystem, SQLite, Codex schema, Git/gh, Wrangler, GitHub, and OTLP checks.

Install OTel at process composition from the first slice: metrics plus parent-based trace-ID ratio, OTLP exporter, payload log/event helper, incident override, and exporter health. Implement the dedicated blocking SQLite actor, pragmas, schema/checksum ledger, exclusive migration lease, pre-migration backup, and schema-newer refusal before domain tables.

Produce a macOS arm64 release binary/tarball/checksum locally. Add
`scripts/tests/00_clean_install.sh` to unpack it into a temporary directory and
drive only public CLI/config/migration/doctor boundaries.

Gate: formatting/check/Clippy pass; a clean directory with no Python/PDM/Cargo on PATH runs version/config/migration/profile/status; a local OTLP receiver observes one full-payload trace at ratio 1.0 and no orphan spans at ratio 0; upgrading a schema-0 and prior-schema fixture to the candidate schema, plus rejecting a future schema, are externally observable.

### Slice 1 — Canonical GitHub Read and Context Projection

Implement GitHub App JWT/installation-token HTTP, typed GraphQL page walkers,
REST supplements, canonical snapshot types, HTML-comment filtering, deterministic
Markdown projection, lifecycle tombstones, Context Revision, pressure, and
Event Reference rendering. Add public diagnostic commands:

- `braid context issue owner/repo#N --config ...`;
- `braid context pr owner/repo#N --config ...`;
- `braid github probe --config ... --repository ...`.

Persist only object/version/tombstone/association metadata needed for later
diffing. A complete query is read twice around the root/association graph and
retries drift; any incomplete connection is a typed unavailable result.

Gate: `scripts/tests/10_context_projection.sh` reads real controlled Issue/PR
objects through the packaged binary and validates deterministic bytes, metadata
ordering, 1:N/N:1 association, open/closed Issue projection, HTML exclusion,
folded/deleted lifecycle, complete pagination, Context Revision stability, and
hard-budget refusal without importing Rust internals.

### Slice 2 — Webhook, Reconciliation, Event Ledger, Reactions, and Tunnel

Implement Axum raw-body ingress/HMAC, typed open webhook enums, durable delivery
ingest before 2xx, reconciliation, canonical diff classification, scheduler
batches/deadlines/count, Braid App reactions, write-outbox convergence, owner
lease, loopback health, Wrangler Quick Tunnel supervision, signed public probe,
and graceful webhook restoration. Provider turns remain disabled in this slice;
runnable batches are visible in status only.

Gate: `scripts/tests/20_ingress_scheduler.sh` uses a real GitHub App/tunnel and
real comments to prove `eyes`, delivery/redelivery dedupe, older-after-newer
non-regression, 30-second quiet reset, eight-event release, trusted mention
permission/Markdown grammar, reconciliation after tunnel loss, and restart of a
pending batch. No Agent comment/turn occurs yet.

### Slice 3 — Profiles, Assignment, Codex Sessions, and Turns

Implement Profile parsing/materialization diagnostics, Braid System Prompt,
Codex NDJSON request/notification ownership, schema pin probe, Issue assignment
generation, `thread/start`, complete Context `thread/inject_items`, idle session,
turn start/steer/interrupt/terminal/unknown, sampled provider transcript, and
trusted-mention lifecycle reactions. Bind scheduler runnable batches to one
active Issue Agent in MVP.

Gate: `scripts/tests/30_issue_agent.sh` uses native Agent App assignment when
that capability exists, otherwise the explicit trusted-mention activation
fallback. It observes an ordinary debounced turn without terminal reactions,
an eight-event turn, a trusted/edited mention steer with rocket/+1, an
unexpected mention terminal with confused, and provider disconnect unknown
with status. The Agent publishes its own attributed comment; Braid mirrors
nothing. The full Slice 3 row is now exercised by Issues #19 and #20 as recorded
in Current Truth.

### Slice 4 — Context Invalidation and Work Item Lifecycle

Implement same-surface Hard Invalidation, Cross-surface description debounce,
Dependency Dirty, stale-output fencing, fresh Codex session replacement,
unassignment debounce, sleeping/retired states, one Finalization Turn, reopen,
Operational Status desired state, and context-pressure blocking.

Gate: the first `scripts/tests/40_context_lifecycle.sh` campaign proves idle and
provider-active Issue-description Hard Invalidation, fresh physical sessions,
stale-turn reaction fencing, and one continuation using current complete
Context. Extend the same public helper with minimize/unminimize, delete,
unassign/reassign, close/reopen/finalization, provider restart, and
context-too-large/unavailable journeys before declaring the whole slice closed.

### Slice 5 — Braid GitHub Writes, PR Ensure, Worktree, and PR Agent

Implement `braid gh` command families and attribution, write receipts,
correlated Agent-origin suppression, uncertain-create reconciliation,
comment-ID-keyed `pr ensure`, native association, deterministic PR Profile
selection, ActivationIntent, PR Agent Group, and one provisioned worktree for the
v1 Implementation Agent. When the chosen remote branch equals base, create the
same-tree Braid App bootstrap commit/ref before the Draft PR, without force.
Implement current multi-Issue PR Context, PR lifecycle, review/review-thread
Event References, and Cross-surface invalidation.

Do not mediate ordinary Git or `gh`; recognize the Profile's configured stable
GitHub actor and expose unconfigured/shared-identity feedback-loop behavior
exactly as Product Truth states.

Gate: `scripts/tests/50_issue_to_pr.sh` runs the real implementation request
twice concurrently, observes one same-tree bootstrap commit plus one Draft
PR/association/worktree/Agent, produces a real verified diff, handles review,
updates Issue design, exercises direct `gh`
under configured and unconfigured identities, closes/reopens/merges with exactly
one finalization turn, and proves no Braid turn mirror exists.

### Slice 6 — Restart, Migration, Packaging, and Operational Convergence

Complete process supervisors, persisted compatible session resume, active-turn
unknown, outbox recovery, App webhook repair/status, graceful/forced shutdown,
release metadata, CHANGELOG workflow, artifact checksums, and upgrade/compatible
rollback. Add optional Collector tail-sampling example without making it a
runtime dependency.

Gate: `scripts/tests/60_operations.sh` runs clean install, pending/active/unknown
restart, tunnel death, uncertain write, DB upgrade/backup/schema-newer refusal,
declared compatible rollback, 10% trace-consistent sampling, incident 100%, and
artifact checksum verification using only public/operator boundaries.

### Slice 7 — Full Acceptance and Release Closure

Run every journey in `docs/10-prd/acceptance.md` on a fresh real fixture three
consecutive times. Preserve the public GitHub evidence, artifact/config/schema/
provider versions, OTLP evidence, process/worktree snapshots, and explicit Human
verdicts. Fix failures in the owning slice, then restart the three-run sequence;
do not weaken the oracle around observed behavior.

After three clean runs: update Product Truth only for proven behavior, finalize
`CHANGELOG.md`, tag/release the macOS arm64 artifact, open the Linux x86_64
follow-on task, delete this packet, and leave no Python/PDM residue.

## Pi Provider Pivot

- **Reason**: Codex account hit its usage limit on 2026-08-23. Real-provider acceptance is blocked until credits are added or the weekly limit resets on 2026-08-27. The user has a working `pi` CLI with a DeepSeek API key and wants to accept against that instead.
- **Implementation Done**:
  1. Introduced `AgentProvider` trait and refactored `CodexClient` into `CodexProvider`.
  2. Added `provider.pi` configuration (executable, provider name, model, API key environment, thinking level, optional home).
  3. Implemented `PiProvider` that spawns `pi --mode rpc`, drives `new_session`/`prompt`/`steer`/`abort`/`get_state`, and maps `turn_start`/`turn_end`/`agent_settled` events to Braid notifications.
  4. Fixed an async runtime panic in `PiProvider::subscribe()` by moving the `broadcast::Sender` out of the async-locked `PiState`.
  5. Fixed `PiProvider` session persistence: switched from `--no-session` to `--session-dir <workspace>/.braid/pi-sessions` so `get_state` returns `sessionFile` as Braid expects.
  6. Updated `config check`, `doctor`, runtime health, and CLI `serve` to use `Box<dyn AgentProvider>`.
  7. Built packaged binary and updated Pi acceptance config `/Users/lanzhijiang/.braid/braid-pi.toml` with `[provider.pi]` and `deepseek-chat` model.
- **Acceptance Status with Pi**:
  - Slice 0 clean-install ✅ passed.
  - Slice 2 transport/scheduler ✅ passed via localtunnel repository-webhook mode.
  - Slice 3 Issue Agent: **partial**. Braid now receives signed webhooks (via a localtunnel + HMAC-signing relay workaround because localtunnel strips GitHub's `X-Hub-Signature-256`), acknowledges the trusted mention with `eyes`, accepts the turn with `rocket`, and starts the Issue Agent session. However, the Pi provider does not have the GitHub tool interface that Braid expects: it cannot publish Agent comments or apply reactions. The `pi --mode rpc` CLI streams reasoning events but does not expose the App-server tool surface that `CodexProvider` uses, so the agent turn never produces the required GitHub side effects. The Slice 3 script times out waiting for the Agent comment/rocket.
- **Blocker**: Pi in RPC mode is a chat/reasoning provider, not an App-server with GitHub tools. Braid's runtime currently relies on the provider to perform all GitHub side effects (comments, reactions, worktree operations). Bridging this would require either (a) giving Pi a custom skill/tool interface that talks back to Braid, or (b) restructuring Braid so the runtime applies provider-intended writes itself. Both are out of scope for PR #94 readiness.
- **Decision Needed**: 
  - Option A: Wait for the Codex usage limit to reset (2026-08-27) and run acceptance against Codex.
  - Option B: Purchase/add Codex credits now.
  - Option C: Invest in a Pi skill/tool bridge to make Pi a full Braid Agent provider.
- **Current Recommendation**: Treat PR #94 as implemented with both provider seams present and verified up to the point where a real provider turn is accepted. Full three-run acceptance still requires a provider with GitHub tool capability; Pi cannot substitute without additional integration work.
