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
braid setup owner/repository --instance <KEY>
```

`--instance` defaults to the repository owner. It is the local name for the
Braid instance and appears in `BRAID_INSTANCE`, the `~/.braid/registry.toml`,
and the `service.instance.id` telemetry attribute.

Options:

- `--provider pi|codex` — defaults to `pi`.
- `--model <MODEL>` — defaults to `deepseek-chat`.
- `--api-key-environment <ENV>` — env var to read the provider API key from
  during setup; defaults to `DEEPSEEK_API_KEY`. The key is then persisted into
  the user-level secrets file, so the env var is only needed at setup time.
- `--user-home <DIR>` — defaults to `~/.braid` (or `BRAID_USER_HOME`).
- `--instance <KEY>` — defaults to the repository owner.

`braid setup` performs these steps:

1. Verifies `gh auth` and reads the acting GitHub login.
2. Resolves the Braid user home (`~/.braid` by default) and instance key
   (owner by default).
3. Generates a random webhook secret.
4. Reads the provider API key from the environment variable named by
   `--api-key-environment`.
5. Builds a GitHub App Manifest with the permissions Braid needs
   (`contents:write`, `issues:write`, `pull_requests:write`, `metadata:read`).
6. Starts a temporary local HTTP callback server.
7. Opens a browser to `github.com/settings/apps/new` (or the organization
   variant) with the manifest pre-filled.
8. Captures the browser redirect and exchanges the code for the App's
   private key, slug, and ID.
9. Writes files under `~/.braid`:
   - `registry.toml` — the instance registry (SSoT for `BRAID_INSTANCE`
     resolution).
   - `instances/<key>/config.toml` (schema version 2)
   - `instances/<key>/github-app.pem` (mode `0600`)
   - `instances/<key>/secrets.toml` (mode `0600`) — webhook secret only
   - `secrets/<provider>.toml` (mode `0600`) — provider API key, shared
     across instances for the same provider
   - `instances/<key>/state/` directory for the SQLite database, backups,
     and worktrees.

Using per-instance secrets and a shared user-level provider key means multiple
Braid instances can run on the same machine without colliding on a single
environment variable. `BRAID_WEBHOOK_SECRET` is no longer required.

After setup:

1. Install the App on your repository by visiting the printed install URL.
2. Run diagnostics:

   ```shell
   braid doctor --instance <KEY>
   ```

3. Start Braid with a public tunnel:

   ```shell
   braid serve --instance <KEY> --tunnel
   ```

   See [`tunnel.md`](tunnel.md) for how the tunnel works and why no
   Cloudflare account is required.

The tunnel receives GitHub webhooks and routes them to Braid's local ingress.

## Provider credentials and workspace

`braid setup` also prepares the instance-scoped provider home
(`~/.braid/instances/<KEY>/provider/codex`) and the Profile workspace
(`~/.braid/instances/<KEY>/workspace/default`).

Codex authenticates per `CODEX_HOME`, and Braid isolates it per instance, so
your global `~/.codex` credentials do not automatically apply. Setup imports
`~/.codex/auth.json` into the instance provider home when it exists; otherwise
authenticate it before serving:

```shell
CODEX_HOME=~/.braid/instances/<KEY>/provider/codex codex login
```

`braid doctor` reports this as the "Codex credentials" check. The Pi provider
needs no home bootstrap: it authenticates with the API key persisted at setup.

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
- The generated App logo is saved as
  `~/.braid/instances/<key>/braid-of-<owner>-logo.png`.
- Secrets are split: the webhook secret lives in the instance
  `secrets.toml`; the provider API key lives in `~/.braid/secrets/<provider>.toml`. Both are mode `0600`.
