# 01 — Filesystem Layout, Registry, Config Schema v2, Secrets, Ports

## Filesystem Layout

```text
~/.braid/                              # BRAID_USER_HOME (dir mode 0700)
  registry.toml                        # instance registry, schema_version = 1
  config.toml                          # optional user defaults; separate schema, not loaded by serve
  secrets/                             # mode 0700
    <provider-id>.toml                 # mode 0600; provider_api_key only
  skills/  mcps/  subagents/           # reserved; created only when a Profile field consumes them
  instances/
    <instance-key>/                    # BRAID_INSTANCE_HOME (dir mode 0700)
      config.toml                      # instance config, schema_version = 2
      github-app.pem                   # mode 0600
      secrets.toml                     # mode 0600; webhook_secret only
      state/                           # runtime.root default
        braid.sqlite3
        backups/
        worktrees/pr-<number>/<profile>-g<generation>/
      provider/<adapter_type>/         # adapter home (e.g. codex home) when local
      logs/
```

Notes:

- `state/` is the default `runtime.root`, so `braid migrate/status` on a loose
  `--config /tmp/x/config.toml` still resolves paths relative to that file.
- `worktrees/` stays under `runtime.root` per the existing TDD wording; only
  the default root moves from `<config_dir>` to `<config_dir>/state`.
- Mutable runtime data lives under `state/` + `provider/` + `logs/`; the
  instance top level holds only config + credentials.

## `registry.toml`

```toml
schema_version = 1
default_instance = "inkcre"        # optional; set to the first registered instance

[[instances]]
key = "inkcre"
home = "instances/inkcre"          # relative to user root, or absolute
github_app_id = 123456
repository = "inkcre/braid"        # MVP: single repo; recorded, not used for routing
```

Rules:

- Registry is the SSoT for instance discovery; instance config files never
  name their own registry entry (they do carry `[instance] key`, see below,
  and `doctor` cross-checks key/app_id/repository agreement).
- `home` relative paths resolve against the user root containing the
  registry. Absolute `home` is allowed (instance living outside `~/.braid`)
  but setup never writes one by default.
- Missing registry file = empty registry (not an error) until a command needs
  an instance, at which point resolution rule 4 errors with guidance to run
  `braid setup`.

## Config Schema v2 (continued; no version bump)

Config schema v2 was introduced by PR #141 and has not shipped in any release,
so this task keeps `CONFIG_SCHEMA_VERSION = 2` and folds its changes in. No
compatibility aliases, no migration:

1. New required section:
   ```toml
   [instance]
   key = "inkcre"
   ```
   Validated with the instance-key charset rule. Purpose: self-describing
   configs for `--config` bypass, telemetry correlation
   (`service.instance.id`), and registry cross-check.
2. `runtime` defaults change:
   - `root` default: `<config_dir>/state` (was `<config_dir>`);
   - `database` default: `<root>/braid.sqlite3` (was `<root>/braid.db`) —
     this also aligns code with `docs/40-deployment/README.md`, which already
     documents `state/braid.sqlite3`;
   - `backups` default: `<root>/backups` (unchanged relative shape).
3. Secret file types split (file formats, not config fields):
   - `github.webhook_secret_file` → `WebhookSecretFile { webhook_secret }`
   - `[[llm_providers]].api_key_file` → `ProviderSecretFile { provider_api_key }`
   The shared `SecretsFile` carrying both is deleted. Error messages name the
   expected key, e.g. `secrets file <path> must contain webhook_secret`.
4. Everything else unchanged: `[[runtimes]]`, `[[llm_providers]]`,
   `[[profiles]]`, `profile_selection`, `scheduler`, `server`, `telemetry`,
   `tools`.

## Library Reuse (happy paths)

Adopted:

- **clap `env` feature**: `#[arg(long, env = "BRAID_INSTANCE")]` style binding
  replaces hand-rolled `std::env::var` plumbing in resolution. This is the
  standard happy path for flag/env equivalence and gives `--help` env
  documentation for free. Same for `BRAID_USER_HOME` / `BRAID_INSTANCE_HOME`.
- **`serde_path_to_error`** around TOML deserialization in `Config::load`,
  registry load, and secrets load: parse errors name the offending dotted
  path instead of a bare line number. Tiny, mature, serde-native.

Evaluated and deferred/rejected:

- **figment / `config` crate (layered merge)**: deferred. The user-level
  `~/.braid/config.toml` is documented as optional defaults but nothing
  consumes it yet; adding a merge framework before a second layer exists is
  speculative. When user defaults are actually merged into instance config,
  figment's serde-native provider model is the candidate.
- **`validator` crate**: rejected. Config validation messages are a CLI
  product surface (exact wording matters for `doctor` output and docs);
  hand-rolled `validate()` keeps that control. The boilerplate it saves is
  small relative to the loss of message control.
- **`camino` (UTF-8 paths)**: deferred. Would clean up `PathBuf` display
  handling, but touches every path field in config/store for no
  user-visible gain in this pass.

## Port Allocation (setup)

- Candidates: ingress `18080 + 2n`, health `18081 + 2n`, for n = 0, 1, 2, …
- A candidate pair is rejected when either port appears in any other registry
  instance's persisted config (best-effort load; unreadable configs are
  skipped with a warning) or when a bind probe on `127.0.0.1:<port>` fails.
- The first accepted pair is persisted into the generated config. First
  instance therefore keeps today's `18080/18081`.
- `doctor` re-checks: for every registry instance, load config and report any
  duplicate ingress/health port across instances, and any port that is
  occupied while the owning instance is not running is left to the existing
  per-config checks (no new liveness tracking).

## Setup Writes

Given `braid setup owner/repo --instance inkcre`:

1. Resolve user root (`--user-home` / `BRAID_USER_HOME` / `~/.braid`), create
   `secrets/` (0700) and `instances/` (0700).
2. Instance dir `instances/inkcre/` (0700) with `state/`, `provider/`,
   `logs/`, `state/backups/`, `state/worktrees/`.
3. `github-app.pem` and `secrets.toml` (webhook secret only), both 0600.
4. Provider key: if `secrets/<provider-id>.toml` exists, reuse it; otherwise
   read `--api-key-environment` and write it (0600). Setup fails with the
   existing install-hint style message if the env var is unset and no file
   exists.
5. Runtime entry home: `instances/inkcre/provider/<adapter_type>`.
6. `config.toml` (schema v2) with allocated ports and absolute file
   references; register/update the instance in `registry.toml` (unique
   key/app_id/home enforced; first instance becomes `default_instance`).
7. Printed next steps use `--instance inkcre`; `--no-browser` manual guide
   prints the same target paths.

## Module Shape (design only; commit plan in 03)

- `src/home.rs` replaces `src/worker.rs`:
  `UserHome` (root resolution, `secrets_dir`, `instances_dir`, registry
  load/save), `Registry`/`InstanceEntry` (schema v1, validation), and
  `resolve_config_path(cli_config, cli_instance, env)` implementing the
  precedence rules from `00`.
- `src/config.rs`: schema v2 delta above; no other restructuring.
- `src/cli/mod.rs`: one shared `ConfigSource { --config, --instance }`
  struct embedded by every config-loading command, replacing `ConfigPath`
  and the bare `--config` fields. Env binding uses clap's `env` feature
  (`BRAID_INSTANCE` etc.), not manual `std::env::var`.
