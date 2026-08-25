# Homebrew + Bootstrap PoC

> **Status: pivoted.** The Homebrew tap, packaged release, and `braid setup` skeleton are in place. The active task packet is now `tasks/code-docs-cleanup.md`.

- **Goal**: Make Braid installable via Homebrew and bootstrappable through a single `braid setup` / `braid bootstrap` flow, so a user can go from zero to a working Braid Issue Agent / PR Agent on their own repository with minimal operator steps.
- **Scope for this PoC**:
  1. Homebrew distribution: a public tap (`xiaoland/homebrew-braid`) with a macOS arm64 formula.
  2. A `braid setup` command that:
     - Detects or initiates `gh auth login`.
     - Generates a GitHub App Manifest for `braid-of-<owner>` and opens the browser for one-click App creation.
     - Starts a temporary local HTTP callback server, captures the manifest `code`, and exchanges it for App credentials.
     - Persists the App ID, PEM private key, webhook secret, and generated `braid.toml` in `~/.braid/`.
     - Creates/updates the repository webhook and verifies a signed `ping`.
     - Starts a persistent public tunnel (localtunnel) for webhook delivery.
     - Generates default issue and PR profiles.
     - Runs `braid doctor` and reports remaining gaps.
- **Guardrails**: Do not place secrets in the repo or conversation. Use env vars and files outside the repo. Keep the provider seam agnostic; the bootstrap can default to Pi if Codex credentials are not present, but must not hardcode a single provider. Use public boundaries and the `gh` CLI wherever possible. Keep system prompts provider-agnostic.
- **Verification**: A clean macOS machine (or fresh VM) can `brew install xiaoland/braid/braid`, run `braid setup --repository owner/repo`, click once in the browser, and end with a passing `braid doctor` and a repository webhook that receives a signed GitHub ping.

## Linear Steps

### Step 1 — Homebrew tap and release artifact
- [x] Create the public tap repository `xiaoland/homebrew-braid`.
- [x] Create a GitHub Release `v0.1.0` on `xiaoland/braid` with the packaged macOS arm64 tarball.
- [x] Write `Formula/braid.rb` with the release URL and checksum.
- [x] Verify `brew install xiaoland/braid/braid` works and `braid --version` prints the expected version.

### Step 2 — `braid setup` skeleton
- [x] Add a new `Setup` command to the CLI (`src/cli.rs`).
- [x] Add an interactive module (`src/setup.rs`) that:
  - Resolves the target owner (user or org) and repository.
  - Checks `gh auth status` and aborts with instructions if not authenticated.
  - Generates a webhook secret (32 bytes hex).
  - Builds the GitHub App Manifest JSON with required permissions/events.
  - Encodes the manifest and opens `https://github.com/settings/apps/new?manifest=...` (or org variant).
  - Starts an ephemeral Axum/HTTP server on a free localhost port and waits for the redirect.
  - Exchanges the `code` via `POST /app-manifests/{code}/conversions`.
  - Writes the PEM to `~/.braid/braid-of-<owner>.pem`, the webhook secret to `~/.braid/braid-of-<owner>.webhook_secret`, and the TOML config to `~/.braid/braid.toml`.
  - Provides `--no-browser` guidance with the manifest URL, JSON, webhook secret, and install URL.

### Step 3 — Tunnel and webhook verification
- [ ] In setup, start a localtunnel (or Cloudflare Quick Tunnel) and obtain the public URL.
- [ ] Create the repository webhook via the GitHub API with the public URL + `/webhook` and the secret.
- [ ] Trigger or await a `ping` delivery, verify HMAC signature, and confirm Braid health is ready.
- **PoC compromise**: setup currently prints the install URL, expects the user to install the App manually, and leaves `braid serve --tunnel` to create the public webhook. Full automation is deferred.

### Step 4 — Profiles and doctor
- [x] Generate default `issue-pi` / `pr-pi` profiles (or Codex if configured) in the generated TOML.
- [ ] Run `braid doctor` equivalent diagnostics inline and print a concise report.
- [x] Print the final `braid serve --config ~/.braid/braid.toml` command.

### Step 5 — Documentation
- [x] Update `README.md` with `brew install xiaoland/braid/braid` and `braid setup --repository owner/repo`.
- [x] Add a short `docs/setup.md` describing the one-time bootstrap flow.

## Done / Closed

Delete this packet after a user other than the author runs the complete flow successfully on a fresh machine.
