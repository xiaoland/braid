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
| R3 | Runtime setup must not modify the user's machine | No auto-install: adapter discovery probe → list candidates → user selects; if empty, print adapter install command (pnpm preferred, npm fallback) and exit; connection config supports `executable_path` or `api_url` (e.g. deepseek-harness) |
| R4 | Schema hash pins currently user-configured (drift source) | Pins are produced/verified by the adapter's connection verifier; user config only stores what `braid setup` persisted after verification |
| R5 | `handoff/slice7-wip` is historical leftover | No action; branch left untouched |
| R6 | Breaking config vs released v0.2.3 | CHANGELOG breaking entry; migration = re-run `braid setup` |
| R7 | Codex OAuth home moves into worker folder | Copy or re-login on first serve; document in CHANGELOG |

### Branch convergence

Git side is settled (clean cut from `main`; stale local branches deleted;
`handoff/slice7-wip` untouched as historical leftover). The remaining
convergence work is on the **execution path itself**: resolve every ambiguity
up front so implementation never backtracks. The pre-resolved specifics below
are binding; deviating requires updating this packet first.

## Pre-resolved Specifics (binding)

### Core trait (`src/agent_session.rs`, new)

```rust
pub enum SessionStatus { Idle, Running, Failed }

#[async_trait::async_trait]
pub trait AgentSession: Send + Sync {
    fn id(&self) -> &str;
    fn status(&self) -> SessionStatus;
    fn status_stream(&self) -> watch::Receiver<SessionStatus>;
    async fn send_user_msg(
        self: &Arc<Self>,
        msg: String,
        steering: bool,
        reset_context_to: Option<String>,
    ) -> Result<Arc<dyn AgentSession>, SessionError>;
}
```

### Worker CLI surface

- `braid setup --worker <name>`, `braid serve --worker <name>`, `braid doctor
  --worker <name>`; each still accepts `--config <path>` as low-level override.
- Worker folder resolution lives in a new `src/worker.rs`; `runtime.root`,
  `runtime.database`, `runtime.backups` become `Option<PathBuf>` defaulting
  inside the worker folder.

### Config TOML (schema_version bumps 1 → 2)

```toml
[[runtimes]]
id = "pi"
adapter = "pi"                 # braid-internal adapter id
version = "0.84.3"             # informational pin from verification
executable_path = "..."        # OR api_url = "http://127.0.0.1:PORT"
# api_url used by HTTP-serving runtimes such as deepseek-harness

[[llm_providers]]
id = "deepseek"
protocol = "openai-compatible"
api_key_file = "secrets.toml"  # relative to worker folder
  [[llm_providers.models]]
  model_id = "deepseek-v4-pro"
  input_cost = 0.0; output_cost = 0.0; cache_input_cost = 0.0
  [[llm_providers.allowances]]
  since = "2026-01-01"; until = "2027-01-01"; amount = 0.0  # metadata only

[[profiles]]
id = "default"
scopes = ["issue", "pr"]       # renamed from tags
adapter = "pi"                 # -> runtimes.adapter
provider = "deepseek"          # -> llm_providers.id
model = "deepseek-v4-pro"      # -> llm_providers.models.model_id
# display_name, priority, user_instructions, reasoning, sandbox, workspace,
# github_actor_node_id, status_surfaces, context pressure fields unchanged
```

Load-time validation: profile.adapter must match a `runtimes` entry;
profile.provider+model must resolve in `llm_providers`; unknown references are
config errors.

### Setup flow (no auto-install)

1. Run every adapter's discovery probe; collect candidates with versions.
2. If candidates exist: list them, user selects one, adapter verifies
   connection (version/schema handshake), persist runtime + default profile.
3. If none: print the adapter's install command (pnpm preferred, npm fallback)
   and exit non-zero with instructions; user re-runs setup after installing.
4. Manual entry path: `--runtime-executable <path>` / `--runtime-api-url <url>`
   flags bypass discovery.

### Module touch list per commit

| Commit | Files |
| --- | --- |
| A | `src/worker.rs` (new), `src/cli/mod.rs`, `src/config.rs`, `src/doctor.rs` |
| B | `src/config.rs`, `config.example.toml`, `config/setup.template.toml`, `src/setup.rs`, `src/doctor.rs`, config tests |
| C | `src/setup.rs`, adapter discovery in `src/provider/{codex,pi}.rs`, `src/protocol.rs` |
| D | `src/agent_session.rs` (new), `src/provider/{mod,codex,pi}.rs`, `src/runtime/scheduler.rs`, `src/runtime/pr_agent.rs` |
| E | `src/runtime/*` renames/reorganization, `docs/20-product-tdd/*` |
| F | `scripts/tests/*.sh`, `CHANGELOG.md`, docs promotion |


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

### C. Setup: discovery + guided manual install
6. Adapters gain discovery probes; `braid setup` lists discovered runtimes,
   verifies the selected one, persists runtime entry + default profile.
   Empty discovery prints the adapter's install command (pnpm preferred, npm
   fallback) and exits with instructions. No auto-install anywhere.

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
- Automatic runtime installation of any kind.
- Automatic migration from old `braid-of-<owner>.*` files.
