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
- For the Pi adapter: an explicitly selected LLM provider, model, and
  credential (`api_key`, `oauth`, or `none`) described below. Codex uses its
  own provider home instead.

## Pi 原生插件

需要 subagents 或 background 时，先按 [原生材料说明](../../native/pi/README.md)
安装锁定材料，再给 setup 指定 `--runtime-executable <install>/pi`。
Setup 识别相邻 `runtime-manifest.json`；既有配置可将 `[[runtimes]]` 的
`executable` 与 `native_runtime` 指向同一安装。外部 Pi 仍可基础接入，但不会自动
加载宿主扩展。材料验证失败须修复明确的文件或配置漂移，不能静默改走全局 Pi。

## Provider credential

Every Pi setup declares an explicit credential kind for its LLM provider:
`api_key`, `oauth`, or `none`. Braid does not derive a default provider, model,
authentication method, or file name from the adapter. Credential files live in
Braid's own schema under `~/.braid/secrets/<provider>.toml` (mode `0600`);
never put them in `config.toml` or a shell environment variable:

```shell
install -d -m 700 ~/.braid/secrets
${EDITOR:-vi} ~/.braid/secrets/<provider>.toml
chmod 600 ~/.braid/secrets/<provider>.toml
```

An `api_key` file contains `provider_api_key = "..."`. An `oauth` file is a
token set (`access`, `refresh`, `expires` in epoch milliseconds, optional
`account`). A `none` route references no file. The Pi adapter converts these
Braid-schema files at session materialization: `api_key` is injected as
`BRAID_PROVIDER_API_KEY` for the matching `models.json.apiKey` reference,
`oauth` is rendered into the session home's `auth.json`, `none` injects
nothing. Credentials are static secrets; replace the file to rotate.

To migrate an existing Pi `auth.json`, run the one-shot converter instead of
copying it anywhere:

```shell
braid provider import-auth <PROVIDER_ID> --from /path/to/auth.json
```

To migrate an existing configuration, remove `api_key_environment` and
`api_key_file`, add an explicit `credential = { kind, file }` entry, and store
the key in that file. Braid does not import legacy environment variables
automatically.

## Bootstrap

Pi requires an explicit model route, provider endpoint, and credential kind:

```shell
braid setup owner/repository --instance <KEY> --adapter pi --model <MODEL> \
  --llm-provider <PROVIDER_ID> --llm-base-url https://api.example.invalid/v1 \
  --llm-credential-kind api_key --llm-credential-file ~/.braid/secrets/<provider>.toml
```

OAuth routes pass `--llm-credential-kind oauth` with the token-set file; a
keyless route passes `--llm-credential-kind none` and no file. The setup writes
`base_url` plus a single declared model; enrich `[[llm_providers.models]]` in
the generated config when you need capability metadata — the Pi adapter derives
`models.json` from it and rejects template copies.

Codex uses its own authenticated home and no LLM provider arguments:

```shell
braid setup owner/repository --instance <KEY> --adapter codex --model <MODEL>
```

`--instance` defaults to the repository owner. It is the local name for the
Braid instance and appears in `BRAID_INSTANCE`, the `~/.braid/registry.toml`,
and the `service.instance.id` telemetry attribute.

Options:

- `--adapter pi|codex` — selects the Agent runtime adapter; it does not imply
  an LLM provider.
- `--model <MODEL>` — model requested from the selected runtime.
- `--reasoning <LEVEL>` — optional runtime reasoning/thinking level; Braid
  does not choose one by default.
- Pi only: `--llm-provider <ID>` declares the LLM route,
  `--llm-base-url <URL>` its endpoint (rendered into the derived `models.json`),
  and `--llm-credential-kind api_key|oauth|none` plus
  `--llm-credential-file <PATH>` (unless `none`) declare its credential.
- `--user-home <DIR>` — defaults to `~/.braid` (or `BRAID_USER_HOME`).
- `--instance <KEY>` — defaults to the repository owner.

`braid setup` performs these steps:

1. Verifies `gh auth` and reads the acting GitHub login.
2. Resolves the Braid user home (`~/.braid` by default) and instance key
   (owner by default).
3. Generates a random webhook secret.
4. For Pi, validates the explicit credential file when the selected route
   declares `api_key` or `oauth`.
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
   - `instances/<key>/config.toml` (schema version 3)
   - `instances/<key>/github-app.pem` (mode `0600`)
   - `instances/<key>/secrets.toml` (mode `0600`) — webhook secret only
   - `instances/<key>/state/` directory for the SQLite database, backups,
     and worktrees.

Using per-instance webhook secrets and explicit provider key files means
multiple Braid instances can run on the same machine without putting provider
credentials in their shell environments. Instances may share one provider key
file only when you intentionally reference the same file.
`BRAID_WEBHOOK_SECRET` is no longer required.

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

## Activating Braid on an Issue

Braid activates through a **trusted `@braid` mention**: a visible `@braid` in
an issue comment from a repository MAINTAIN/ADMIN actor wakes the Issue Agent
(after the Quiet Window). Braid acknowledges the mention with an `eyes`
reaction.

Native Issue assignment is a GitHub-side *Agent App* provisioning (the same
capability that makes Copilot assignable), not a permission an ordinary
GitHub App can hold — an App created through the manifest flow does not
appear in the assignee picker, and assigning it via the API is rejected. If
GitHub ever provisions your App as an Agent App, assignment works without any
configuration change; Braid detects the mode at runtime.

## Provider credentials and source checkout

`braid setup` also prepares the instance-scoped provider home
(`~/.braid/instances/<KEY>/provider/codex`) and the instance source checkout
(`~/.braid/instances/<KEY>/source`): one Git clone of the configured
repository shared by all Profiles. Braid never edits it directly — Agent
sessions run in dedicated generation-scoped worktrees provisioned from it
(`state/worktrees/...`), and each worktree's `.braid/` directory is git-excluded
scratch space the Agent may use freely. Setup clones
the repository automatically; if the clone cannot run, it prints the manual
`git clone` command and `braid doctor` reports the missing checkout.

Codex authenticates per `CODEX_HOME`, and Braid isolates it per instance, so
your global `~/.codex` credentials do not automatically apply. Setup imports
`~/.codex/auth.json` into the instance provider home when it exists; otherwise
authenticate it before serving:

```shell
CODEX_HOME=~/.braid/instances/<KEY>/provider/codex codex login
```

`braid doctor` reports this as the "Codex credentials" check. Pi needs no
provider-home bootstrap: it authenticates with the explicit credential
declared by its provider entry (`api_key` or `oauth` file, or `none`).

## Multiple repositories in one instance

A setup-generated instance starts with one repository. One GitHub App and one
Braid instance may serve additional explicitly configured repositories when the
operational profile is intentionally shared, but Profiles, workspaces, Events,
worktrees, and writes remain repository-scoped. Braid does not build a shared
cross-repository Context or work graph.

Add repository names under `additional_repositories`, then bind every GitHub
Profile to a repository and give it an explicit source checkout:

```toml
[github]
repository = "owner/first"
additional_repositories = [{ name = "owner/second" }]
# ... App credentials and webhook settings

[[profiles]]
id = "issue-first"
repository = "owner/first"
workspace = "/absolute/path/to/first-source"
# ...

[[profiles]]
id = "issue-second"
repository = "owner/second"
workspace = "/absolute/path/to/second-source"
# ...
```

Profile ids remain unique within the instance. Install the same GitHub App on
every configured repository. Webhooks from other installed but unconfigured
repositories are rejected before scheduling. `braid gh pr ensure` requires
`--repository` in a multi-repository instance; generation-scoped worktrees are
kept under a repository-prefixed path. This release keeps the existing MVP
constraint that all Profiles use the same runtime adapter type; it does not yet
mix Codex and Pi repositories in one instance.

## Headless / manual App creation

If you cannot or do not want to open a browser from the terminal, run:

```shell
braid setup owner/repository --adapter pi --no-browser
```

Braid writes a pending setup draft under `~/.braid/setup-drafts/`, prints the
manifest form, and shows how to finish later. Open the HTML form from any
browser. GitHub redirects to the local callback URL; on a headless host that
page is expected to fail. Copy the complete URL, then run the same setup
options again with `--callback-url` instead of `--no-browser`:

```shell
braid setup owner/repository --adapter pi --model <MODEL> --llm-provider <ID> \
  --callback-url '<complete-redirect-url>'
```

The callback state must match the pending draft, so a stale or cross-instance
redirect is rejected before the App manifest is converted. The completion path
requires no browser on the setup host. It prints:

- A generated auto-submitting HTML form file; open it in any browser to POST
  the manifest to GitHub.
- A `curl` command that performs the same POST.
- The full manifest JSON for copy-paste creation.
- The install URL for the repository.
- Instructions for completing the instance with `--callback-url`, or for manual
  recovery by writing the PEM, secrets file, and config yourself.

GitHub expects the manifest as a `POST` form field named `manifest`; a plain
query parameter will not pre-fill the form. The generated HTML file handles this
automatically.

## Notes

- The GitHub App Manifest flow is the supported path for creating Apps
  programmatically; GitHub does not expose a headless API for App creation.
- The generated App logo is saved as
  `~/.braid/instances/<key>/braid-of-<owner>-logo.png`.
- Secrets are split: the webhook secret lives in the instance
  `secrets.toml`; any Pi provider credential lives in the explicit file
  referenced by `llm_providers[].credential`. Both files should be mode `0600`.
