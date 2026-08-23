# Rust MVP Implementation

- **Goal**: Bring PR #94 — *Rebuild Braid around GitHub working memory* — to a mergeable state. The Rust runtime, GitHub Context, lifecycle, PR Agent worktree, `braid gh`, migrations, packaging, and operational convergence are already implemented through schema v11. The only remaining PR-blocker is to run and pass three clean full acceptance campaigns against real GitHub/Codex. This packet is now oriented toward *PR readiness* rather than chasing every Slice 7 external fault path before the PR itself is aligned.
- **Objective**: Replace the withdrawn Python turn-mirror prototype with the packaged Rust MVP specified by the Product Truth, Product TDD, provider/GitHub contracts, deployment contract, and real black-box acceptance oracle.
- **Guardrails**: GitHub Context is canonical working memory; do not preserve Python mirror/thread assumptions or add compatibility aliases. Implement Codex only while keeping the documented provider seam. Do not constrain Agent `gh`/`git` beyond GitHub/provider/Profile permissions. Treat digest/hash as an exceptional internal mechanism, not a product or domain primitive: first repair the owning identity, version, lifecycle, or authority contract at the preceding architecture layer. Do not place hashes, UUIDs, delivery IDs, provider/session/turn/item IDs, or SQLite IDs in Agent Context; unavoidable internal identifiers remain transport-local and semantically inert. No GitHub/network await inside SQLite transactions. No internal fake/unit/component test suite; public diagnostic and campaign helpers live under `scripts/tests/`. Sampled telemetry intentionally preserves full evidence and is treated as sensitive. Keep this as the only active task packet and delete it after the three clean acceptance campaigns and durable release evidence close the task.
- **Verification**: `cargo fmt --check`, `cargo check --locked --all-targets`, and strict Clippy diagnose every slice. Each slice also has its public-boundary gate below. Product acceptance requires the clean packaged macOS arm64 artifact to pass every journey in `docs/10-prd/acceptance.md` three consecutive times without corrective operator action; internal state/logs never substitute. Linux x86_64 packaging is the immediate follow-on gate, not a blocker for the first macOS MVP release.
- **Handoff Baseline (2026-08-19)**: The working tree is clean at commit `8504cd6` (`feat(runtime): converge PR lifecycle and operations`), plus one task-packet commit on `feat/docs-namespace-migration`. `cargo fmt --check`, `cargo check --locked --all-targets`, and `cargo clippy --locked --all-targets` all pass. The previous agent's in-progress uncertain-write test harness (modified `scripts/tests/60_operations.sh`, `scripts/tests/README.md`, `src/github.rs`, and new `scripts/tests/connect_delay_proxy.py`) was moved to a WIP branch `handoff/slice7-wip` so the baseline branch remains known-good. The questionable `GitHubError::Response` → `is_unavailable()` change was reverted with that WIP; `Response` means a deterministic response-shape error and must not be treated as an uncertain transport outcome.
- **Realignment**: The active PR is the real target. The branch name `feat/docs-namespace-migration` no longer matches the PR or the work. The next actions are: rename the branch to a name that matches the PR, update the PR body to reflect the current schema/work, and treat Slice 7/acceptance as the *remaining gate* for that PR — not as a separate side quest. Uncertain-write proxy harnesses, tunnel-death reruns, and Context-unavailable fixtures stay on the `handoff/slice7-wip` branch until the PR is aligned and acceptance credentials are available.
- **Current Truth**: Product, Context, lifecycle, Codex, GitHub, SQLite/OTel/deployment, and acceptance designs are closed in their durable owners. Slices 0–5 are implemented and have bounded real public-boundary evidence. Schema v11 now carries Issue and PR lifecycle uniformly: a PR receives one Finalization Turn on close, sleeps without granting ordinary work, reopens into a fresh provider session while preserving its worktree, and receives one final Finalization Turn before retiring on merge. Canonical merged state cannot be downgraded by the Issue-shaped `closed` state in later PR conversation comments. Unconfigured direct Agent comments remain external wake input; a configured stable direct actor remains a separate capability gap. Slice 6 process supervision, compatible resume, active-turn neutral unknown, bounded shutdown, owner fencing, uncertain comment reconciliation, Quick Tunnel candidate verification/repair, packaging, migration/rollback, and trace-consistent OTel behavior are implemented. An ordinary GitHub App is not a standard assignable user, so the PoC uses the first trusted `@braid` on a dormant Issue as ActivationIntent plus its initial Wake; native assignment/unassignment and Context-unavailable acceptance remain honest capability gaps. Braid publishes no turn mirror.
- **Current Slice Evidence**: The expanded Slice 4 campaign passed against closed Issue #28. In addition to the Issue #27 Context lifecycle evidence, it closed the Issue only after public status proved an active provider turn, observed that turn complete normally, observed exactly one completed reaction-free Finalization Turn, and then observed `sleeping`. A closed-state comment received `eyes` but granted no second turn. Reopen created a fresh physical provider session from complete current GitHub Context and released one ordinary Wake only after the normal debounce; its Agent comment proved the closed-state comment body was present. The temporary repository webhook count returned to zero, Issue #28 was closed, the campaign's remote App key was revoked, the downloaded key was destroyed, and the secret-free temporary config directory was removed from `/tmp` to Trash. Three older App private-key fingerprints predate this campaign and were not deleted without separate ownership evidence.
- **Latest Evidence**: The schema-v7 `scripts/tests/40_context_lifecycle.sh` campaign passed through real GitHub, an HTTP/2 Quick Tunnel, packaged Braid, and pinned Codex against closed Issues #29–#31. After the accepted lifecycle matrix, it terminated the idle app-server, observed Braid reconnect and resume the same provider thread, and proved that thread handled the next ordinary debounced Wake. A soft-pressure Context was supplied in full, completed its Agent turn, and converged to one App-authored status; the hard-pressure fixture created no provider session or turn and supplied no partial Context. All three Issues are closed, the repository webhook count returned to zero, the campaign key was revoked, and the isolated worktree/runtime was removed from `/tmp` to Trash.
- **Slice 5 Evidence**: The final schema-v11 `scripts/tests/50_issue_to_pr.sh` campaign passed without operator correction against closed Issue #86, merged PR #87, and closed active-restart fixture Issue #88. Concurrent `pr ensure` calls converged on one PR; the Codex Agent pushed only `acceptance/slice5-20260815T100919Z.md`. Review Wake/thread resolution, Associated-Issue active invalidation, direct external origin, close/sleep/no-extra-turn/reopen, same-worktree idle restart, disposable-base merge, two total Finalization Turns, merge retirement, and absence of a Braid turn mirror all converged. Restarting Braid during the separate active Issue turn preserved the same provider session and one neutral `unknown` turn, started no parallel work, left the non-terminal reaction intact, and published one Operational Status Comment. Cleanup closed both Issues, removed the merged fixture refs, temporary hook, source checkout, runtime, and tunnel.
- **Slice 6 Evidence**: `scripts/tests/60_operations.sh` passed packaging/checksum/clean-install, schema 0→11 and 1→11 backups, schema-compatible prior-binary read, graceful owner release, forced-exit fencing and post-expiry restart, trace-consistent 10% sampling (4 of 30 roots), and incident-mode 100% sampling. The same run reported `tunnel=unavailable`: repeated account-less Quick Tunnel candidates registered locally but their public routes timed out from both Braid and system curl. Braid correctly kept health in `verifying`, did not patch the App webhook, discarded unreachable candidates, and exited boundedly. Runtime-owned tunnel child-death repair and an actual remote-success/response-loss uncertain comment remain unproven external fault journeys; the convergence implementations are present but must not be called accepted yet.
- **Environment Setup (2026-08-23)**:
  - GitHub App `braid-by-xiaoland` inspected via pi-chrome: App ID `4558000`, installation `153412294`, permissions `contents=write`, `issues=write`, `metadata=read`, `pull_requests=write`; required webhook events enabled.
  - New App private key generated/downloaded through GitHub UI and stored at `/Users/lanzhijiang/.braid/braid-by-xiaoland.pem` with `0600` permissions.
  - Webhook secret set and exported as `BRAID_WEBHOOK_SECRET`.
  - Codex CLI `@openai/codex@0.147.0-alpha.6.5` installed at `~/.braid/codex-pkg/node_modules/.bin/codex`; provider home seeded from existing `~/.codex` auth to `~/.braid/provider`.
  - Schema digests verified: stable `7d79fe…`, experimental `a14d48…`.
  - Clean source checkout for PR worktree at `~/.braid/source/braid`.
  - Acceptance config written to `/Users/lanzhijiang/.braid/braid.toml`; `config check`, `doctor` (except OTLP), `github probe`, `migrate apply`, and `context issue` diagnostics all pass.
  - Packaged release artifact built: `/Users/lanzhijiang/.braid/dist/braid-v0.1.0-aarch64-apple-darwin.tar.gz`.
  - Fixed stale schema assertions in `scripts/tests/30_issue_agent.sh` and `scripts/tests/40_context_lifecycle.sh` (expected schema 10 → 11).
  - Background campaigns are now running; results will be appended as they complete.
- **Tunnel workaround**: Cloudflare Quick Tunnel public routes intermittently return Cloudflare error 1033 (edge cannot resolve tunnel). To unblock acceptance, installed `localtunnel` and patched scripts 20/30/40/50 to accept `BRAID_TEST_PUBLIC_WEBHOOK_URL` with any `https://*/webhook` host. A persistent localtunnel is running at `https://braid-acceptance-test-lan.loca.lt`.
- **Campaign Progress**:
  - Slice 0 clean-install ✅ passed.
  - Slice 2 transport/scheduler ✅ passed via localtunnel repository-webhook mode.
  - Slice 3 Issue Agent: blocked by Codex usage limit. Braid correctly receives webhooks, creates the Issue Agent session, accepts the trusted-mention turn, and starts the provider turn, but Codex returns: "You've hit your usage limit. Upgrade to Pro ... or try again at Aug 27th, 2026 12:56 PM." The ChatGPT/Codex usage page confirms "You're out of Codex and Work usage for now".
  - Slices 4–6 cannot run until Codex usage is available.
- **Next Steps / Decision Needed**: Purchase Codex Pro/add credits, or wait for the Aug 27 reset, before the remaining real-provider acceptance campaigns can pass.
- **Next Step — Route A: Realign and run acceptance**:
  1. **Branch/PR alignment** (done): renamed to `feat/rust-working-memory`, created PR #94, closed/superseded PR #2.
  2. **Run each public campaign at least once** to confirm the environment, then run all journeys three consecutive times cleanly. Fix failures in the owning slice/test and restart the sequence; do not weaken the oracle.
  3. **Release closure** (after three clean runs): update Product Truth only for proven behavior, finalize `CHANGELOG.md`, tag/release the macOS arm64 artifact, open the Linux x86_64 follow-on task, delete this packet, and leave no Python/PDM residue.

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
- **Implementation Plan**:
  1. Introduce a provider trait (`AgentProvider`) and refactor `CodexClient` into the first implementation.
  2. Add `provider.pi` configuration (executable, provider name, model, API key environment, thinking level).
  3. Implement `PiProvider` that spawns `pi --mode rpc`, drives `prompt`/`steer`/`abort`, and maps `turn_start`/`turn_end` events to Braid notifications.
  4. Update `config check`, `doctor`, runtime health, and CLI `serve` provider selection.
  5. Switch the acceptance config and campaigns to Pi + DeepSeek and rerun Slices 3–6.
