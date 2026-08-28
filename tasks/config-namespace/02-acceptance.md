# 02 — Acceptance Plan

This pass changes internal persistence and CLI resolution only. The product
acceptance oracle in `docs/10-prd/acceptance.md` is untouched; GitHub-visible
behavior must not change. Evidence below is compile/unit/shell level.

## Unit Tests (Rust)

`src/home.rs`:

1. Resolution precedence: `--config` > `BRAID_INSTANCE_HOME` > `--instance` >
   `BRAID_INSTANCE` > registry default > single instance > ambiguous error
   listing keys > empty-registry error pointing at `braid setup`.
2. Registry round-trip: save/load preserves entries; loading rejects duplicate
   keys, duplicate `github_app_id`, duplicate `home`; unknown key error lists
   known keys.
3. Instance key validation: accepts owner-derived defaults (`inkcre`,
   `my-org`); rejects empty, uppercase, separators, leading/trailing `-`,
   >39 chars.
4. Relative `home` resolves against the user root; absolute `home` kept as-is.
5. Port allocation: skips pairs used by other registry configs, skips
   unbindable pairs (test binds a blocker listener), first free pair is
   `18080/18081` on a clean slate.

`src/config.rs`:

6. v2 loads with the new required `[instance] key`; missing key errors;
   invalid key errors.
7. Runtime defaults resolve to `<config_dir>/state`, `state/braid.sqlite3`,
   `state/backups/`; explicit values still honored.
8. Schema 1 and 3 files rejected with the unsupported-schema message
   (version stays 2).
9. Secrets split: webhook file with only `webhook_secret` loads; provider file
   with only `provider_api_key` loads; cross-loaded file (wrong key) errors
   naming the expected key.
10. `config::tests::example_config_matches_schema` updated for the new v2
    shape.

## Shell Suite

Existing scripts keep passing `--config` (unchanged surface for them):

- `scripts/tests/00_clean_install.sh` — update the generated fixture to the
  new v2 shape (`[instance] key`, `state/` layout); assert the fixture DB
  lands at `state/braid.sqlite3`.
- `10/20/30/40/50/60` — unchanged except any fixture-config regeneration and
  the schema assertion helper.

New `scripts/tests/05_instances.sh` (hermetic, no GitHub access; hand-writes
registry + two minimal v2 instance configs under a temp `BRAID_USER_HOME`):

1. Bare `braid config check` with one instance resolves it.
2. Two instances + `default_instance` → bare commands use the default;
   `--instance other` selects the other; `BRAID_INSTANCE=other` selects the
   other.
3. Two instances, no default → bare command fails and the error lists both
   keys.
4. Precedence: `BRAID_INSTANCE_HOME` beats `BRAID_INSTANCE`; `--config` beats
   `BRAID_INSTANCE_HOME`.
5. `braid doctor --json` flags a hand-crafted duplicate ingress port across
   the two instances.
6. Secret split observable: instance `secrets.toml` contains no
   `provider_api_key`; config check fails with the expected-key error if the
   webhook file is cross-loaded as a provider key file.
7. Legacy paths are inert: a stray `~/.braid/workers/<name>/config.toml`
   (under the temp home) is never read by any resolution path.

## Full Gates

- `cargo fmt && cargo check && cargo clippy --all-targets && cargo test` green
  at every commit.
- `scripts/tests/00_clean_install.sh` and `scripts/tests/05_instances.sh`
  green on the final commit; the GitHub-dependent suites (10–60) are
  unaffected in behavior and remain the PR-level evidence they already are.

## Explicit Non-Goals

- No migration tooling for old paths (re-run `braid setup`).
- No skills/MCP/subagent schema or directory population.
- No multi-repository serving; registry records the single repository only.
- No changes to GitHub-visible behavior, Context rendering, scheduling, or
  provider contracts.
