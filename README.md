# GitHub Agent Bridge

This directory is an independent prototype project. It has its own SVC markers,
task packets, Python package, dependency lock, and test commands. It is not a
member of the parent repository's PDM workspace and must not change the parent
lockfile, package imports, CI, or product corpus.

## Local Setup

Python 3.12 and PDM 2.28 or later are required.

```shell
pdm install -G test
pdm run test
```

Run the real, read-only provider contract probe with explicit absolute paths:

```shell
pdm run github-agent-bridge probe-app-server \
  --codex /absolute/path/to/codex \
  --workspace /absolute/path/to/a/read-only-probe-directory
```

Copy `config.example.json` to the ignored `config.local.json`, replace every
placeholder, create the state database's parent directory, and validate it:

```shell
pdm run github-agent-bridge config-check --config /absolute/path/to/config.local.json
```

Start loopback ingress plus periodic canonical reconciliation without changing
the GitHub App webhook URL:

```shell
pdm run github-agent-bridge serve \
  --config /absolute/path/to/config.local.json \
  --repository owner/repository \
  --issue-number 123
```

For a dedicated test GitHub App, add the free Wrangler Quick Tunnel. The
runtime reports the temporary public URL, updates the App webhook while it is
running, and restores the previous URL on a graceful stop:

```shell
pdm run github-agent-bridge serve \
  --config /absolute/path/to/config.local.json \
  --repository owner/repository \
  --issue-number 123 \
  --wrangler /absolute/path/to/wrangler
```

The Quick Tunnel exposes only the webhook app. Bounded runtime health remains
loopback-only at `http://127.0.0.1:<health_port>/healthz`.

`serve` fails closed when the installed Codex version or either generated
schema digest differs from the configured protocol pin. The collaboration
instructions path is only an integrity pin: the persistent workflow lives in
this project's `AGENTS.md`, where Codex loads it only for this project. The provider cwd
must be a pre-provisioned, dedicated Issue worktree; the Wrapper launches there
but never creates, selects, or manages its branch, worktree, or PR.
The proposed bounded section is documented in
[`docs/project-scope-collaboration.md`](docs/project-scope-collaboration.md) and
installed in this project's `AGENTS.md`; ordinary chat outside this project is
unaffected.

The local runtime and Quick Tunnel supervisor are developer-verified building
blocks, not product acceptance. No GitHub App, webhook, Issue, Cloudflare
resource, external instruction, or external comment has been created by the
implementation work. The real Issue-to-Draft-PR black-box campaign remains the
only acceptance path.

See [the protocol contract](docs/app-server-protocol.md) and the live
[implementation packet](tasks/bootstrap-implementation.md). External setup and
exclusive handoff are specified in the [operator runbook](docs/operator-runbook.md).
