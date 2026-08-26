# Braid Setup

Braid is distributed as a prebuilt macOS arm64 binary through a Homebrew tap.
One `braid setup` command bootstraps your own GitHub App, persists credentials
outside the repository, and writes a starter configuration.

## Install

```shell
brew install xiaoland/braid/braid
braid --version
```

## Requirements

- macOS arm64 (Apple Silicon).
- [`gh`](https://cli.github.com/) installed and authenticated with an account
  that can create GitHub Apps for the target owner (user or organization).
- A repository where you will install the App.
- For the Pi provider: a DeepSeek API key exported as `DEEPSEEK_API_KEY` (or
  the environment variable configured with `--api-key-environment`) *only at
  setup time*.

## Bootstrap

```shell
export DEEPSEEK_API_KEY=...
braid setup owner/repository
```

Options:

- `--provider pi|codex` — defaults to `pi`.
- `--model <MODEL>` — defaults to `deepseek-chat`.
- `--api-key-environment <ENV>` — env var to read the provider API key from
  during setup; defaults to `DEEPSEEK_API_KEY`. The key is then persisted into
  the per-owner secrets file, so the env var is only needed at setup time.
- `--home <DIR>` — defaults to `~/.braid`.

`braid setup` performs these steps:

1. Verifies `gh auth` and reads the acting GitHub login.
2. Generates a random webhook secret.
3. Reads the provider API key from the environment variable named by
   `--api-key-environment`.
4. Builds a GitHub App Manifest with the permissions Braid needs
   (`contents:write`, `issues:write`, `pull_requests:write`, `metadata:read`).
5. Starts a temporary local HTTP callback server.
6. Opens a browser to `github.com/settings/apps/new` (or the organization
   variant) with the manifest pre-filled.
7. Captures the browser redirect and exchanges the code for the App's
   private key, slug, and ID.
8. Writes per-owner files under `~/.braid`:
   - `~/.braid/braid-of-<owner>.pem` (mode `0600`)
   - `~/.braid/braid-of-<owner>.secrets.toml` (mode `0600`)
   - `~/.braid/braid-of-<owner>.toml`

Using a per-owner secrets file means multiple Braid instances for different
owners can run on the same machine without colliding on a single environment
variable. `BRAID_WEBHOOK_SECRET` is no longer required.

After setup:

1. Install the App on your repository by visiting the printed install URL.
2. Run diagnostics:

   ```shell
   braid doctor --config ~/.braid/braid-of-<owner>.toml
   ```

3. Start Braid with a public tunnel:

   ```shell
   braid serve --config ~/.braid/braid-of-<owner>.toml --tunnel
   ```

   See [`tunnel.md`](tunnel.md) for how the tunnel works and why no
   Cloudflare account is required.

The tunnel receives GitHub webhooks and routes them to Braid's local ingress.

## Headless / manual App creation

If you cannot or do not want to open a browser from the terminal, run:

```shell
braid setup owner/repository --no-browser
```

This prints:

- A generated auto-submitting HTML form file; open it in any browser to POST
  the manifest to GitHub.
- A `curl` command that performs the same POST.
- The full manifest JSON for copy-paste creation.
- The install URL for the repository.
- Instructions on how to persist the PEM, secrets file, and config so Braid
  can run.

GitHub expects the manifest as a `POST` form field named `manifest`; a plain
query parameter will not pre-fill the form. The generated HTML file handles this
automatically.

## Notes

- The GitHub App Manifest flow is the supported path for creating Apps
  programmatically; GitHub does not expose a headless API for App creation.
- Secrets never live in the repository; they are stored in
  `~/.braid/braid-of-<owner>.secrets.toml` with mode `0600`.
- The starter configuration defaults to the Pi provider. Switch to Codex by
  passing `--provider codex` if you have a Codex environment ready.
