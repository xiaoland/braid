# Rust MVP Implementation

- **Objective**: Replace the withdrawn Python turn-mirror prototype with the packaged Rust MVP specified by the Product Truth, Product TDD, provider/GitHub contracts, deployment contract, and real black-box acceptance oracle.
- **Guardrails**: GitHub Context is canonical working memory; do not preserve Python mirror/thread assumptions or add compatibility aliases. Implement Codex only while keeping the documented provider seam. Do not constrain Agent `gh`/`git` beyond GitHub/provider/Profile permissions. No GitHub/network await inside SQLite transactions. No internal fake/unit/component test suite; public diagnostic and campaign helpers live under `scripts/tests/`. Sampled telemetry intentionally preserves full evidence and is treated as sensitive. Keep this as the only active task packet and delete it after the three clean acceptance campaigns and durable release evidence close the task.
- **Verification**: `cargo fmt --check`, `cargo check --locked --all-targets`, and strict Clippy diagnose every slice. Each slice also has its public-boundary gate below. Product acceptance requires the clean packaged macOS arm64 artifact to pass every journey in `docs/10-prd/acceptance.md` three consecutive times without corrective operator action; internal state/logs never substitute. Linux x86_64 packaging is the immediate follow-on gate, not a blocker for the first macOS MVP release.
- **Current Truth**: Product, Context, lifecycle, Codex, GitHub, SQLite/OTel/deployment, and acceptance designs are closed in their durable owners. Slices 0 and 1 are complete. The dedicated Braid App installation on `xiaoland/braid` authenticated the packaged binary and the real `scripts/tests/10_context_projection.sh` gate passed against Issue #1, closed Issue #3, Draft PR #2, and secondary Draft PR #4. Public evidence covers deterministic Markdown bytes, App identity/permissions, native 1:N and N:1 associations, sorted metadata, open/closed Issue projection, actual HTML-comment exclusion, minimized/deleted lifecycle, real multi-page GraphQL reads through diagnostic page size one, stable Context Revision, and hard-budget refusal. Forward-only schema v2 retains deletion tombstones; embedded queries also passed GitHub's live schema against public `openai/codex` objects. Slice 2 is implemented at schema v3. The real `scripts/tests/20_ingress_scheduler.sh` behavioral campaign passed through GitHub, an HTTP/2 Quick Tunnel, packaged Braid, and `braid-by-xiaoland[bot]` against closed Issues #5–#9: durable ingress, App-owned `eyes`, delivery rededuplication, older-after-newer non-regression, 30-second debounce reset, eight-event release, trusted visible mention grammar, reconciliation during tunnel loss, and pending-batch restart all converged. Slice 3 is complete at schema v5. Candidate `braid 0.1.0` SHA-256 `aef6c630de8563b7d86ddfa1af6e6d85c1cb38a67bccedd4560f096c587e7fb2` passed the real `scripts/tests/30_issue_agent.sh` campaign through GitHub, HTTP/2 Quick Tunnel, packaged Braid, and pinned Codex. Closed Issue #19 proves dormant trusted-mention activation, expected-turn edit steer, `eyes`/`rocket`→`eyes`/`+1`, an ordinary turn after the full 30-second debounce, eight durably received events releasing one threshold turn, Agent-authored attributed comments, zero Braid turn mirrors, and app-server loss preserving unknown/`rocket` plus one independent Operational Status Comment. Closed Issue #20 proves an accepted intentionally unsupported-model turn receiving real Codex `turn.failed`, with observed `rocket` converging to `confused` and no fabricated Agent comment. Diagnostic run #17 correctly failed because the operator supplied a stale host binary rather than the newly packaged candidate; the helper now records candidate SHA/schema. Run #18 showed only five of eight GitHub deliveries had reached Braid when its timer began; threshold timing now begins after eight public `eyes` prove durable receipt. Slice 4 remains in progress at schema v6. Candidate `braid 0.1.0` SHA-256 `fb10186ce2dcd4310d3a29ac063e34c71977efa91333ba679a0d7e5f3fe50d38` passed the expanded `scripts/tests/40_context_lifecycle.sh` through real GitHub, an HTTP/2 Quick Tunnel, packaged Braid, and pinned Codex against closed Issue #27. It preserves the accepted idle/active description invalidation and stale-turn reaction fence, then proves minimize through reconciliation removes the body and replaces the idle session without fabricating a turn; unminimize restores the body, releases one ordinary Wake, and reuses that valid session; delete retains a body-less tombstone and replaces the session without fabricating a turn. Public status records exactly five physical sessions and zero Braid turn mirrors. The campaign also found and fixed a real defect: GitHub updates Issue `updatedAt` when comments are added, so root change detection now compares the human-visible root Context projection rather than treating activity time as a description edit. Aborted diagnostic Issues #25 and #26 captured the old misclassification and an invalid command-style sleep fixture respectively; both are closed. All temporary repository webhooks are deleted and the one-time local App private key/secret were destroyed. A live standard-assignee probe returned HTTP 403 for `braid-by-xiaoland[bot]`: ordinary GitHub Apps are not assignable users, so the PoC uses the first trusted `@braid` on a dormant Issue as ActivationIntent plus its initial Wake. Native Agent App assignment remains a capability-dependent branch rather than a claimed pass. App webhook bootstrap/handoff also remains explicitly unavailable.
- **Next Step**: Complete the remaining Slice 4 lifecycle matrix on the accepted reset state machine: debounced unassignment and lifecycle sleeping/retired/reopen/finalization behavior; provider restart/status convergence; and context-pressure Operational Status. Extend the same public helper only when each runtime seam exists, and keep every not-yet-run journey explicitly unaccepted. Preserve the unavailable native Agent App assignment and App webhook bootstrap branches as honest capability results.

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
