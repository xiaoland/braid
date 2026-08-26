# Setup MVP Quality

- **Goal**: Bring `braid setup` to a level where a first-time user can install,
  bootstrap their own GitHub App, and start `braid serve` without hitting
  avoidable schema, UX, or documentation errors.
- **PR**: #140 `feat(setup): bring braid setup to MVP quality`
- **Branch**: `feat/setup-mvp`

## Done

- [x] Fix generated config missing required `[[profiles]]` and
  `[profile_selection]` by generating the file from the canonical `Config` type.
- [x] Validate the generated config before writing.
- [x] Add unit tests that round-trip generated TOML through `Config::load` for
  both `pi` and `codex` providers.
- [x] Treat `config.example.toml` as the canonical starter template and validate
  it against `Config`.
- [x] Document the SSoT schema pattern in `docs/20-product-tdd/README.md`
  (including ROI boundary and the SQLite migration exception).
- [x] **Per-owner config**: `braid setup` writes
  `~/.braid/braid-of-<owner>.toml` instead of a shared `~/.braid/braid.toml`.
- [x] **Unified per-owner secrets file**: `braid setup` writes
  `~/.braid/braid-of-<owner>.secrets.toml` containing webhook secret and
  provider API key; `Config` supports `github.webhook_secret_file` and
  `provider.pi.api_key_file`; `BRAID_WEBHOOK_SECRET` and provider env vars are
  no longer required at runtime.

## Open / To Be Discovered

- [ ] **App logo**: `braid-of-<owner>` should visually brand the App. GitHub App
  Manifest does not support a logo field, so a logo must be uploaded after
  App creation via the GitHub API. Proposed: fetch the owner avatar, composite
  the Braid logo at the bottom-right, handle light/dark/transparency, and
  upload it.
- [ ] **Tunnel explanation / setup UX**: users may think `serve --tunnel`
  requires a Cloudflare account. Clarify that Wrangler Quick Tunnel uses the
  free `trycloudflare.com` service without an account, and that `serve --tunnel`
  automatically starts the tunnel, verifies reachability, and updates the GitHub
  App webhook URL to the public tunnel URL.
- [ ] **Collect and fix other setup UX issues** found during real end-to-end
  runs.
- [ ] **Update `docs/user-manual/setup.md`** once the final flow stabilizes.
- [ ] **Release a new version** after the setup fixes are complete.

## Verification

- `cargo fmt/check/clippy/test`
- `braid setup <owner>/<repo> --no-browser` produces valid manifest and config
- `braid doctor --config ~/.braid/braid-of-<owner>.toml` passes without setting
  environment variables
- `braid serve --config ~/.braid/braid-of-<owner>.toml --tunnel` starts without
  config parse errors

Delete this packet when PR #140 is merged into the release line.
