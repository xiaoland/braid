# Implementation Plan

Single PR on branch `feat/agent-architecture` (task packet rides along).
Ordered as sequential commits, each keeping `cargo fmt/check/clippy/test`
green. Product behavior is preserved; only internal architecture moves.

## Rehearsal Findings

### Current architecture map

- `src/config.rs`: `provider.codex/pi` per-runtime sections; Profile has
  `tags`, `provider: String`, no `adapter`; `runtime.root/database/backups`
  are mandatory absolute paths.
- `src/provider/mod.rs`: `AgentProvider` is a **process-level** trait keyed by
  `thread_id` (start/resume/inject_context/start_turn/steer/interrupt) plus a
  process-wide `broadcast<ProviderNotification>`.
- `src/runtime/scheduler.rs` + `pr_agent.rs`: ~10 call sites drive sessions;
  context reset (`materialize_context_reset`) is done **by the scheduler**, not
  the provider — this logic moves into the adapter.
- `src/store/mod.rs`: quiet window / event threshold / pending batches already
  exist as store machinery — the Event Queue is a reorganization, not new
  mechanism.
- `src/setup.rs`: **hardcoded machine-specific paths and pins**
  (`/Users/lanzhijiang/Library/pnpm/bin/pi`, codex executable path, version
  string, schema sha256). This is a portability bug the runtime registry must
  fix.

### Risk register

| # | Risk | Mitigation |
| --- | --- | --- |
| R1 | Config schema is breaking (`deny_unknown_fields`); example/template/setup/doctor must move atomically | Existing SSoT round-trip tests catch drift; update all four in one commit |
| R2 | AgentSession trait refactor touches ~10 scheduler call sites | Shim-first: wrap existing `AgentProvider` behind the new trait, then rewire call sites, then move reset ownership |
| R3 | On-demand runtime install needs a package mechanism | codex/pi are npm packages; install via `npm install --prefix <worker>/runtimes/<name> <pkg>@<pin>`; if npm absent, error with manual instructions and keep executable-path override |
| R4 | Schema hash pins currently user-configured (drift source) | Pins move into the Braid-managed registry entry (known-good pins ship with the binary) |
| R5 | `handoff/slice7-wip` branch has an uncertain-write test harness not in main | **Needs owner decision**: merge, rebase later, or drop |
| R6 | Breaking config vs released v0.2.3 | CHANGELOG breaking entry; migration = re-run `braid setup` |
| R7 | Codex OAuth home moves into worker folder | Copy or re-login on first serve; document in CHANGELOG |

### Branch convergence

- Done: deleted local `feat/setup-mvp`, `feat/rust-working-memory` (content
  already in main via squash).
- Pending: `handoff/slice7-wip` (see R5).

## Execution Order (sequential commits in one PR)

### A. Worker folder layout
1. `--worker <name>` flag on setup/serve/doctor; resolve config/secrets/db/
   runtimes/worktrees/logs from `~/.braid/workers/<name>/`.
2. `runtime.root/database` become optional with worker-folder defaults;
   `--config` stays as low-level override.

### B. Config schema
3. Profile: `tags` → `scopes`, add `adapter`; keep `github_actor_node_id`,
   `status_surfaces`, context-pressure fields.
4. Add `llm_providers` (metadata-only allowances) and runtime registry tables;
   profiles resolve provider+model at load time.
5. Update `config.example.toml`, `config/setup.template.toml`, `setup.rs`
   generation, `doctor.rs`, and SSoT tests atomically.

### C. Setup + runtime install
6. `braid setup` asks for default agent runtime, creates default profile, and
   installs the pinned runtime into `runtimes/` on demand; remove hardcoded
   paths from `setup.rs`.
7. `braid serve` verifies pinned runtimes, installs missing ones.

### D. AgentSession trait
8. Define core trait (`send_user_msg`, `status`, `status_stream` via
   `tokio::sync::watch`); implement codex/pi adapter shims over existing
   `AgentProvider`.
9. Rewire scheduler call sites onto the trait.
10. Move physical session replacement / context reset into the adapter
    (`reset_context_to`); collapse the scheduler-side reset path.

### E. Event producer / queue / group naming
11. Reorganize ingress → Event Producer, store-backed debounce → Event Queue,
    batch sender → thin Agent Group; update module names and docs.

### F. Verification
12. Update `scripts/tests/*.sh`, run the shell suite against a local worker;
    CHANGELOG breaking-change entry; promote packet decisions into
    `docs/20-product-tdd/`.

## Out of scope (this pass)

- Parallel symmetric fan-out (multiple sessions per group).
- LLM allowance enforcement.
- `braid runtime install` standalone command.
- Automatic migration from old `braid-of-<owner>.*` files.
