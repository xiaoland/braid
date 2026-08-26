# Implementation Plan

Ordered as incremental, independently mergeable PRs. Each PR keeps
`cargo fmt/check/clippy/test` green and preserves existing product behavior.

## PR 1 — Worker folder layout

- Introduce `~/.braid/workers/<name>/` with `config.toml`, `secrets.toml`,
  `braid.db`, `runtimes/`, `worktrees/`, `logs/`.
- `braid setup --worker <name>` writes into the folder; `braid serve --worker
  <name>` resolves config/secrets/db from it.
- Keep `--config <path>` as a low-level override.
- Make `runtime_root` / `database` optional with worker-folder defaults.
- `braid doctor` validates the worker folder shape.

## PR 2 — Profile / provider / runtime config schema

- Profile: rename `tags` → `scopes` (issue|pr), add `adapter` reference; keep
  `github_actor_node_id`, `status_surfaces`, context pressure fields.
- Add global `llm_providers` table (protocol, connection, `api_key_file`,
  models with costs, allowances as metadata only).
- Add runtime registry table (per worker): adapter type, pinned version,
  install path, checksum.
- Profiles reference `provider` + `model` into `llm_providers`; resolution is
  validated at config load (unknown provider/model = config error).
- Round-trip tests: generated setup config and `config.example.toml` validate
  against `Config` (extend the existing SSoT tests).

## PR 3 — Setup flow: runtime selection + default profile

- `braid setup` asks which default agent runtime (codex app-server / pi),
  creates the default agent profile referencing it, and installs that runtime
  into `runtimes/` on demand.
- `braid serve` verifies pinned runtimes exist and installs missing ones before
  starting.
- No default runtime install without a profile that references it.

## PR 4 — AgentSession trait + adapter refactor

- Core trait: `send_user_msg(msg, steering, reset_context_to) -> AgentSession`
  (returns immediately), `status()`, `status_stream()` via
  `tokio::sync::watch`.
- Adapter owns physical session replacement/fork internally; caller never
  branches on status before sending.
- Refactor existing codex/pi providers behind the trait without changing
  behavior; `recv()` stays adapter-internal for debugging.

## PR 5 — Event producer / queue / group wiring

- Event Producer: webhook/GraphQL ingress → classify → explicit routing to
  (work-item, agent-group) queues; Agent-origin events dropped for the
  originating group.
- Event Queue: per work-item per agent-group; owns quiet window/threshold;
  emits batch (user message text + optional new context + steering).
- Agent Group: thin forwarder — holds the session handle created at
  activation, calls `send_user_msg(...)`; one session per group in MVP.

## Out of scope (this pass)

- Parallel symmetric fan-out (multiple sessions per group).
- LLM allowance enforcement.
- `braid runtime install` standalone command.
- Automatic migration from old `braid-of-<owner>.*` files.
