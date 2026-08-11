<p align="center">
  <img src="docs/assets/braid-logo-128.png" width="128" height="128" alt="Braid logo">
</p>

# Braid

**Braid** keeps a GitHub Issue and one local Coding Agent thread in a durable
collaboration loop. This independent prototype has its own SVC markers, task
packets, Python package, dependency lock, and test commands.

## Local Setup

Python 3.12 and PDM 2.28 or later are required.

```shell
pdm install -G test
pdm run test
```

Run the real, read-only provider contract probe with explicit absolute paths:

```shell
pdm run braid probe-app-server \
  --codex /absolute/path/to/codex \
  --workspace /absolute/path/to/a/read-only-probe-directory
```

Copy `config.example.json` to the ignored `config.local.json`, replace every
placeholder, create the state database's parent directory, and validate it:

```shell
pdm run braid config-check --config /absolute/path/to/config.local.json
```

Start loopback ingress plus periodic canonical reconciliation without changing
the GitHub App webhook URL:

```shell
pdm run braid serve \
  --config /absolute/path/to/config.local.json \
  --repository owner/repository \
  --issue-number 123
```

For a dedicated test GitHub App, add the free Wrangler Quick Tunnel. The
runtime reports the temporary public URL, updates the App webhook while it is
running, and restores the previous URL on a graceful stop:

```shell
pdm run braid serve \
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

Each Agent turn is one visible GitHub comment: assistant messages and
provider-labelled reasoning summaries remain readable, while every supported
tool call uses a compact `<summary>` with bounded call/result evidence folded
inside `<details>`. The final assistant response is promoted to the top of the
same comment. Braid never places debug JSON, protocol IDs, raw chain-of-thought,
or an ownership marker in the GitHub body.

The local runtime and Quick Tunnel supervisor remain building blocks, not full
product acceptance. The bounded historical smokes and their findings are
recorded in [`tasks/minimal-mirror-smoke.md`](tasks/minimal-mirror-smoke.md) and
[`tasks/human-readable-turn-mirror.md`](tasks/human-readable-turn-mirror.md).
Their disposable GitHub objects were removed during project extraction; the
task packets preserve the evidence and diagnosed failure modes. The full
Issue-to-Draft-PR black-box campaign remains separate.

See [the protocol contract](docs/app-server-protocol.md), the
[projection reducer contract](tasks/projection-reducer-contract.md), and the
[implementation packet](tasks/bootstrap-implementation.md). External setup and
exclusive handoff are specified in the [operator runbook](docs/operator-runbook.md).
