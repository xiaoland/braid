# Worker Layout

## Goal

Consolidate everything a Braid worker owns into one folder, replacing the
current scattered `~/.braid/braid-of-<owner>.*` files.

## Decisions

One worker = one folder:

```
~/.braid/workers/<worker-name>/
  config.toml      # was braid-of-<owner>.toml
  secrets.toml     # mode 0600
  braid.db         # SQLite local state
  worktrees/       # PR implementation worktrees
  logs/            # runtime / tunnel logs
```

- Runtimes are **not** stored here: Braid never installs runtimes (see
  `03-runtime-registry.md`); `executable_path` / `api_url` point wherever the
  user installed them.

- CLI: `braid serve --worker <name>` (and `setup --worker <name>`) replaces
  `--config <path>` as the primary entry; `--config` stays as a low-level
  override.
- `runtime_root` / `database` config fields default to paths inside the worker
  folder and become optional.
- Migration from the old `braid-of-<owner>.toml` layout is a one-time `braid
  setup` re-run; no automatic data migration in this pass.
