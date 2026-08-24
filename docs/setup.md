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
  the environment variable configured with `--api-key-environment`).

## Bootstrap

```shell
braid setup owner/repository
```

Options:

- `--provider pi|codex` — defaults to `pi`.
- `--model <MODEL>` — defaults to `deepseek-chat`.
- `--api-key-environment <ENV>` — defaults to `DEEPSEEK_API_KEY`.
- `--home <DIR>` — defaults to `~/.braid`.

`braid setup` performs these steps:

1. Verifies `gh auth` and reads the acting GitHub login.
2. Generates a random webhook secret.
3. Builds a GitHub App Manifest with the permissions Braid needs
   (`contents:write`, `issues:write`, `pull_requests:write`, `metadata:read`).
4. Starts a temporary local HTTP callback server.
5. Opens a browser to `github.com/settings/apps/new` (or the organization
   variant) with the manifest pre-filled.
6. Captures the browser redirect and exchanges the code for the App's
   private key, slug, and ID.
7. Writes:
   - `~/.braid/braid-of-<owner>.pem` (mode `0600`)
   - `~/.braid/braid-of-<owner>.webhook_secret` (mode `0600`)
   - `~/.braid/braid.toml`

After setup:

1. Install the App on your repository by visiting the printed install URL.
2. Export the webhook secret:

   ```shell
   export BRAID_WEBHOOK_SECRET=$(cat ~/.braid/braid-of-<owner>.webhook_secret)
   ```

3. Run diagnostics:

   ```shell
   braid doctor --config ~/.braid/braid.toml
   ```

4. Start Braid with a public tunnel:

   ```shell
   braid serve --config ~/.braid/braid.toml --tunnel
   ```

The tunnel receives GitHub webhooks and routes them to Braid's local ingress.

## Headless / manual App creation

If you cannot or do not want to open a browser from the terminal, run:

```shell
braid setup owner/repository --no-browser
```

This prints:

- A pre-filled GitHub App Manifest URL you can paste into a browser.
- The full manifest JSON for copy-paste creation.
- The generated webhook secret.
- The install URL for the repository.
- Instructions on how to persist the PEM, secret, and config so Braid can run.

Use this when setting Braid up on a remote machine, CI, or any environment
where `open`/`xdg-open` is not available.

## Notes

- The GitHub App Manifest flow is the supported path for creating Apps
  programmatically; GitHub does not expose a headless API for App creation.
- Secrets never live in the repository; they are stored in `~/.braid`.
- The starter configuration defaults to the Pi provider. Switch to Codex by
  passing `--provider codex` if you have a Codex environment ready.
