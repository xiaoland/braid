# Config Namespace and Persistence Layout

Status: active design packet. Do not promote to PRD/TDD until the scope model and
CLI/env surface are explicitly closed in this packet.

## Decision Direction

Use a single Braid user root plus explicit serve instances.

- `BRAID_USER_HOME`: shared local Braid root. Default `~/.braid`.
- `BRAID_INSTANCE`: logical serve-instance key resolved under the user root.
- `BRAID_INSTANCE_HOME`: explicit filesystem override for one instance home.
  This replaces the old overloaded `BRAID_HOME` idea. No backward compatibility.

Preferred CLI surface:

- `--instance <KEY>` selects `~/.braid/instances/<KEY>/config.toml` through the
  registry.
- `--config <PATH>` remains the exact debug/power-user override.
- `BRAID_INSTANCE_HOME` is an env-only advanced override; if set, the instance
  config path is `$BRAID_INSTANCE_HOME/config.toml` unless `--config` wins.

## Scope Model

The namespace is not a simple owner tree. Use these boundaries:

| Scope | Root / key | Owns | Process boundary |
| --- | --- | --- | --- |
| Host | OS user, PATH, loopback ports | discovered executables, free ports, local machine constraints | no persistent Braid authority |
| Braid User Home | `BRAID_USER_HOME`, default `~/.braid` | registry, user defaults, provider-account secrets, shared capability assets once implemented | not a serve process |
| Braid Instance | `BRAID_INSTANCE` / `BRAID_INSTANCE_HOME` | one `braid serve`: GitHub App credentials, webhook identity, SQLite/outbox/scheduler, logs, worktrees | yes; one serve per instance |
| GitHub App | App ID + private key + webhook secret | authentication and webhook trust | credential scope; MVP instance key may default to owner, but registry must record App ID |
| Installation / Repository | GitHub installation and repository node IDs | canonical Work Items, routing, reconciliation cursors | data/routing scope inside an instance, not a default filesystem root |
| Provider Account | LLM provider id | provider API keys and model catalogue facts | user-level secret reference, not per-owner copy by default |
| Profile | `[[profiles]]` in instance config | adapter/model/reasoning/instructions/status surfaces and future explicit skill/MCP/subagent enablement | composition scope, not a filesystem root |
| Session / Turn / Worktree | instance `provider/`, `state/`, `worktrees/` | replaceable execution state | ephemeral/recoverable, not authority |

## Target Layout

```text
~/.braid/                         # BRAID_USER_HOME
  registry.toml                   # instances, default_instance, app_id/repo metadata
  config.toml                     # optional user defaults; separate schema from instance Config
  secrets/                        # user-level provider secrets, mode 0600
  skills/ mcps/ subagents/        # only when Profile fields consume them
  instances/
    <instance-key>/               # BRAID_INSTANCE_HOME
      config.toml                 # instance Config; Rust types remain SSoT
      github-app.pem              # mode 0600
      secrets.toml                # mode 0600; at least GitHub webhook secret
      state/braid.sqlite3
      state/backups/
      provider/
      worktrees/<owner>/<repo>/<pr>-<generation>/
      logs/
```

## Known Current Defects to Fix

- `src/setup.rs` writes fixed loopback ports `127.0.0.1:18080` and
  `127.0.0.1:18081`; multiple instances cannot serve concurrently.
- `src/worker.rs` still models `~/.braid/workers/<name>`; this task replaces it
  with user-root/instance resolution.
- docs still mention flat `~/.braid/braid-of-<owner>.toml`; remove that model.
- LLM provider keys are currently written per worker by setup; move the default
  to user-level `secrets/<provider>.toml` referenced by instance config.
- Do not add shared skills/MCP/subagent schema before a `Profile` field consumes
  it; reserve the directories and ownership rules only.

## Open Questions

1. MVP instance key: default to repository owner (`inkcre`) while registry stores
   `github_app_id` and `repository`, or default to `app-<id>` with a friendly
   alias?
2. Registry format: single `registry.toml` SSoT vs scanning
   `instances/*/config.toml` plus optional default pointer.
3. Port allocation: setup probes and persists free ingress/health ports; doctor
   cross-checks all registry instances for collisions.
4. Multi-repo future: keep `github.repository` in instance config for MVP, but
   do not encode repo as the instance-home key; evolve to multiple repositories
   only when routing is implemented.
5. Secret placement: GitHub App PEM/webhook secret stay instance-local; provider
   API keys default to user-level secrets and are referenced by path.

## Close Condition

Promote only after this packet explicitly records: final env/CLI names, instance
key rule, registry schema, target layout, port allocation rule, secret ownership
rule, and the config schema version bump. Then delete this packet.
