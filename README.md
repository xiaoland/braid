<p align="center">
  <img src="docs/assets/braid-logo-128.png" width="128" height="128" alt="Braid logo">
</p>

# Braid

Braid turns GitHub Issues and pull requests into durable working memory for
local Coding Agents. GitHub holds the current design, implementation state,
metadata, relationships, and discussion; Braid rebuilds a provider session from
that state whenever prior context becomes stale.

Braid is implemented as one Rust package and one portable `braid` binary. The
active product and implementation contracts are:

- [Product Truth](docs/10-prd/README.md)
- [Real end-to-end acceptance](docs/10-prd/acceptance.md)
- [Rust Product TDD](docs/20-product-tdd/README.md)
- [GitHub Context](docs/20-product-tdd/context.md)
- [Event/session lifecycle](docs/20-product-tdd/lifecycle.md)
- [Codex provider contract](docs/20-product-tdd/app-server.md)
- [GitHub boundary](docs/20-product-tdd/github.md)
- [Deployment and observability](docs/40-deployment/README.md)
- [Glossary](glossary.md)

The first supported delivery target is a packaged macOS arm64 binary; Linux
x86_64 follows. Build and inspect the public operator surface with:

```shell
cargo build --locked
cargo run --locked -- --version
cargo run --locked -- config check --config /absolute/path/to/braid.toml
cargo run --locked -- migrate plan --config /absolute/path/to/braid.toml
cargo run --locked -- github probe --config /absolute/path/to/braid.toml --repository owner/repository
cargo run --locked -- context issue owner/repository#123 --config /absolute/path/to/braid.toml
cargo run --locked -- serve --config /absolute/path/to/braid.toml --tunnel
cargo run --locked -- status --config /absolute/path/to/braid.toml --json
```

Apply all pending migrations before `context`; the local canonical ledger keeps
only mechanical versions, associations, and deleted-comment tombstones while
GitHub remains the content authority.
The diagnostic `--page-size` defaults to GitHub's maximum of 100; real campaign
helpers may lower it to force pagination while requiring byte-identical Context.
`serve --tunnel` owns only transport in the current slice: verified webhook
ingress, canonical reconciliation, reactions, and runnable debounce batches.
It does not start a Coding Agent turn until the provider slice is enabled.

Copy [`config.example.toml`](config.example.toml) outside the checkout and
replace every placeholder path before running diagnostics. A packaged release
does not require Python, PDM, Cargo, or a source checkout.

Braid deliberately avoids a large internal fake/unit-test surface while the
workflow is being established. Diagnostic and real black-box campaign helpers
belong under [`scripts/tests/`](scripts/tests/), but retained GitHub/provider/
OTel evidence and Human verdicts remain the acceptance oracle.
