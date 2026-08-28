# 00 — Scope Model, Env/CLI Surface, Resolution Rules

All questions from the packet kickoff are closed here. Reasoning rule used
throughout: persistence layout follows **trust boundary × sharing scope ×
durability**, not the GitHub owner string.

## Scope Inventory

| Scope | Root / key | Owns | Process boundary | Notes |
| --- | --- | --- | --- | --- |
| Host | OS user, PATH, loopback ports | executable discovery, free-port probing | none | never persisted as authority; probed at setup/doctor time |
| Braid User Home | `BRAID_USER_HOME`, default `~/.braid` | registry, optional user defaults, provider-account secrets, future shared skills/MCP/subagents | none; not a serve process | private to the OS user despite being "shared across instances" |
| Braid Instance | `BRAID_INSTANCE` key; home under `instances/<key>/` | one GitHub App credential set, webhook identity, SQLite + outbox + scheduler, provider homes, worktrees, logs | **yes** — exactly one `braid serve` per instance | the only durable runtime boundary |
| GitHub App | `github_app_id` in registry | App private key, webhook secret, webhook URL ownership | credential scope of its instance | one App belongs to exactly one instance (webhook URL is app-level; two instances would fight over it) |
| Installation / Repository | GitHub node IDs in SQLite | canonical Work Items, routing, sync cursors | data scope inside an instance | never a default filesystem root; GitHub Projects v2 is metadata, not a scope |
| Provider Account | `[[llm_providers]]` id | provider API keys, model catalogue | user-level secret, referenced by path | keys are not owner-specific; do not copy per instance |
| Profile | `[[profiles]]` in instance config | adapter/model/reasoning/instructions/status surfaces; future explicit skill/MCP/subagent enablement | composition scope | references shared assets; GitHub content never selects assets |
| Session / Turn / Worktree | instance `state/` + `provider/` | replaceable execution state | ephemeral | not authority; rebuilt from GitHub + SQLite |

## Env and CLI Surface

Environment variables:

| Variable | Meaning | Default |
| --- | --- | --- |
| `BRAID_USER_HOME` | Braid user root | `~/.braid` |
| `BRAID_INSTANCE` | instance key for commands that need one | none |
| `BRAID_INSTANCE_HOME` | explicit instance home override (advanced) | none |

`BRAID_HOME` is intentionally absent. The old name conflated user root and
instance home; the new pair makes the two scopes unambiguous and symmetric.

CLI:

- All config-loading commands take one shared source struct:
  `--config <PATH>` (exact file) or `--instance <KEY>` (registry lookup).
  This replaces both today's `ConfigPath { --config, --worker }` and the
  bare required `--config` on context/gh/probe commands.
- `braid setup OWNER/REPOSITORY` gains `--user-home <PATH>` (default
  `$BRAID_USER_HOME` or `~/.braid`) and `--instance <KEY>` (default: owner).
  `--home` and `--worker` are deleted.
- No new subcommands for MVP. `registry.toml` is human-editable; setup prints
  the resulting registration.

## Resolution Precedence (identical for every command)

1. `--config <PATH>` — exact file, registry bypassed entirely.
2. `BRAID_INSTANCE_HOME` — config is `$BRAID_INSTANCE_HOME/config.toml`;
   registry bypassed.
3. `--instance <KEY>` / `BRAID_INSTANCE` — registry lookup; unknown key is a
   hard error listing known keys.
4. Registry default: `default_instance` if set; else the single registered
   instance; else a hard error. When multiple instances exist and no default
   applies, the error lists all keys.

## Instance Key Rule

- Setup default: the repository owner from `OWNER/REPOSITORY`, lowercased.
- Validation: 1–39 chars, lowercase ASCII alnum plus `-`, no leading/trailing
  `-`, no path separators. This is a subset of GitHub login rules, so the
  owner-derived default always validates.
- `--instance <KEY>` overrides the default; the key is a local name only.
- Registry invariants, checked at registration and by `doctor`:
  - `key` unique;
  - `github_app_id` unique across instances (one App = one instance);
  - `home` unique across instances.

## Non-Goals for This Design

- No migration from `~/.braid/workers/<name>` or flat `braid-of-<owner>.*`.
  Dev machines re-run `braid setup`.
- No skills/MCP/subagent asset schema yet: no `Profile` field consumes them
  today. This design only fixes their ownership rule (user root, enabled
  per-profile) and reserves the directories.
- No multi-repository serving. `github.repository` stays singular; the
  registry records it so a future multi-repo instance does not need to move
  its home.
