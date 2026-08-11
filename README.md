<p align="center">
  <img src="docs/assets/braid-logo-128.png" width="128" height="128" alt="Braid logo">
</p>

# Braid

**Braid** keeps a GitHub Issue and one local Coding Agent thread in a durable
collaboration loop. This independent prototype has its own SVC integration,
Python package, dependency lock, and operator commands.

## Local Setup

Python 3.12 and PDM 2.28 or later are required.

```shell
pdm install
pdm lock --check
pdm run braid --help
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
The bounded section is installed in this project's `AGENTS.md`; ordinary chat
outside this project is unaffected.

Each Agent turn is one visible GitHub comment: assistant messages and
provider-labelled reasoning summaries remain readable, while every supported
tool call uses a compact `<summary>` with bounded call/result evidence folded
inside `<details>`. The final assistant response is promoted to the top of the
same comment. Braid never places debug JSON, protocol IDs, raw chain-of-thought,
or an ownership marker in the GitHub body.

The local runtime and Quick Tunnel supervisor remain building blocks, not full
product acceptance. The product promise and its real Issue-to-Draft-PR oracle
are defined in [`docs/10-prd/README.md`](docs/10-prd/README.md) and
[`docs/10-prd/acceptance.md`](docs/10-prd/acceptance.md).

Braid deliberately has no unit or component-integration test suite at this
stage. Executable acceptance helpers belong under
[`scripts/tests/`](scripts/tests/) only when they drive the real product through
public boundaries. They are workflow tools, not product acceptance by
themselves; the retained GitHub evidence and Human verdicts remain the oracle.

See [the architecture](docs/20-product-tdd/README.md), [Codex protocol
contract](docs/20-product-tdd/app-server.md), [GitHub transport
contract](docs/20-product-tdd/github.md), and [turn projection
contract](docs/20-product-tdd/turn-projection.md). External setup and exclusive
handoff are specified in the [operator runbook](docs/40-deployment/README.md).
