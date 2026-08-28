# 03 — Implementation Plan

Single PR, stacked. Every commit keeps `cargo fmt/check/clippy/test` green.

## Branch Strategy

- This work depends on `feat/agent-architecture` (it introduced
  `src/worker.rs` and the module layout this plan deletes/rewrites).
- Branch `feat/config-namespace` from the tip of `feat/agent-architecture`;
  open a stacked draft PR. After PR #141 merges, rebase onto `main`.
- The packet directory `tasks/config-namespace/` rides along and is deleted in
  the final commit after docs promotion.

## Commit Order

### 1. `src/home.rs` — user root, registry, resolution

- Add `src/home.rs` (`UserHome`, `Registry`, `InstanceEntry`, key validation,
  `resolve_config_path` with the precedence rules, port-pair allocator).
- Delete `src/worker.rs`; swap `mod worker;` → `mod home;` in `src/main.rs`.
- Unit tests from `02` items 1–5 land here.
- `src/cli/mod.rs` temporarily wires resolution through `home` while keeping
  flag names; renamed in commit 2.

### 2. CLI surface — `--instance` everywhere, delete `--worker`

- Introduce one shared `ConfigSource { --config <PATH>, --instance <KEY> }`
  in `src/cli/mod.rs`; embed it in `ConfigPath` users (config check, migrate,
  status, doctor) **and** the commands with bare required `--config`
  (profile inspect, telemetry probe, tunnel probe, context, github probe /
  redeliver, gh comment/pr). Those become source-based with the same
  precedence.
- `ServeArguments`: `--config` / `--instance`.
- `SetupArguments`: add `--user-home <PATH>`, `--instance <KEY>`; delete
  `--home`, `--worker`.
- Env: `BRAID_USER_HOME`, `BRAID_INSTANCE`, `BRAID_INSTANCE_HOME` honored in
  `resolve_config_path` / setup root resolution.

### 3. Config schema v3

- `src/config.rs`: `CONFIG_SCHEMA_VERSION = 3`; `[instance] key` (required,
  validated); runtime defaults `<config_dir>/state`, `braid.sqlite3`; split
  `SecretsFile` into `WebhookSecretFile` / `ProviderSecretFile` with
  expected-key error messages; update validation and `ConfigSummary` (include
  instance key).
- `config.example.toml` regenerated to v3 with the new layout comments.
- Unit tests from `02` items 6–10.

### 4. Setup writes the new layout

- `src/setup.rs`: user root + instance dirs (0700/0600), split secrets,
  user-level provider key reuse, `provider/<adapter_type>` runtime home,
  port-pair allocation, config emission (schema v3), registry
  registration/update with uniqueness enforcement, first instance becomes
  `default_instance`, printed next steps and `--no-browser` guide updated.
- `src/setup/discovery.rs` untouched.

### 5. Telemetry instance tagging

- `src/telemetry.rs`: add `service.instance.id = config.instance.key` to the
  OTel resource alongside `service.name`. No pipeline changes.

### 6. Shell suite

- `scripts/tests/00_clean_install.sh`: v3 fixture + `state/braid.sqlite3`
  assertions.
- New `scripts/tests/05_instances.sh` per `02`.
- `scripts/tests/README.md` index update.
- Sweep 10–60 only where fixture config generation/assertion helpers need the
  v3 shape.

### 7. Docs and packet close

- `docs/user-manual/setup.md`, `docs/user-manual/tunnel.md`: new layout,
  `--instance`, secrets ownership, multi-instance section.
- `docs/40-deployment/README.md`: filesystem layout section rewritten to the
  instance layout; startup/config bullets updated for schema v3.
- `docs/20-product-tdd/README.md`: module table entry `worker` → `home`;
  config bullet notes user root + instance split.
- CHANGELOG: breaking-change entry (paths, flags, env, schema v3).
- Delete `tasks/config-namespace/` in this commit.

## File Touch List (expected)

- New: `src/home.rs`, `scripts/tests/05_instances.sh`.
- Deleted: `src/worker.rs`.
- Edited: `src/main.rs`, `src/cli/mod.rs`, `src/cli/helpers.rs`,
  `src/config.rs`, `src/setup.rs`, `src/telemetry.rs`, `src/doctor.rs` /
  `src/cli/doctor_cmd.rs` (port cross-check), `config.example.toml`,
  `scripts/tests/00_clean_install.sh`, `scripts/tests/README.md`,
  `docs/user-manual/{setup,tunnel}.md`, `docs/40-deployment/README.md`,
  `docs/20-product-tdd/README.md`, `CHANGELOG.md`.

## Risks / Watch Items

- `ConfigSource` conversion touches many CLI structs; keep it mechanical and
  land it as its own commit.
- Port allocator must not hold the probe socket; bind-check-release, then
  persist. First-instance behavior must stay `18080/18081`.
- Doctor's cross-instance port check loads other instances' configs
  best-effort; an unloadable config is a warning, never a failure of the
  instance being checked.
- Setup re-run for an existing instance key updates the registry entry but
  must not regenerate or overwrite existing `github-app.pem` / secrets;
  refuse with a clear message unless the user deletes the instance dir.
