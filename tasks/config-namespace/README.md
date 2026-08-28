# Config Namespace and Persistence Layout

Status: active design packet. Design, acceptance, and implementation plan are
written down in the sub-packets below. Do not promote to PRD/TDD and do not
start implementation until a human confirms the decisions in `00`–`03`.

No backward compatibility: `BRAID_HOME`, `--worker`, `~/.braid/workers/<name>`,
and the flat `~/.braid/braid-of-<owner>.*` layout are all deleted, not migrated.

- **00-scope-model.md** — final scope boundaries, env/CLI surface, resolution
  precedence, instance key rule. All questions closed.
- **01-layout-registry-config.md** — filesystem layout, `registry.toml` schema,
  config schema v3 delta, secrets split, port allocation rule.
- **02-acceptance.md** — observable acceptance: unit tests, shell-suite changes,
  and the explicit non-goals relative to `docs/10-prd/acceptance.md`.
- **03-implementation-plan.md** — branch strategy and commit-by-commit plan with
  file-level touch list.

## Decisions Locked Here

1. One user root `~/.braid` (`BRAID_USER_HOME`); serve instances live under
   `instances/<key>/`. No per-owner top-level dotdirs.
2. Env surface: `BRAID_USER_HOME`, `BRAID_INSTANCE`, `BRAID_INSTANCE_HOME`.
   `BRAID_HOME` does not exist.
3. CLI surface: `--instance <KEY>` (registry lookup) and `--config <PATH>`
   (exact override). `--worker` is gone.
4. Instance = one `braid serve` = one GitHub App credential/webhook + one
   SQLite/outbox/scheduler. MVP instance key defaults to the repository owner;
   registry records `github_app_id` as the durable identity.
5. Repository stays a data/routing scope inside an instance, never a default
   filesystem root.
6. GitHub App PEM + webhook secret are instance-local; LLM provider keys are
   user-level secrets referenced by path.
7. Config schema bumps to v3: required `[instance] key`, runtime defaults move
   to `<config_dir>/state` with `braid.sqlite3`.
8. Setup allocates loopback ports deterministically (18080+2n, skipping
   registry-used and unbindable ports) instead of hard-coding 18080/18081.

## Close Condition

After human review of `00`–`03`, implement per `03`, run the acceptance in
`02`, promote binding truth into `docs/20-product-tdd/` and
`docs/40-deployment/`, then delete this packet.
