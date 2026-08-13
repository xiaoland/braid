<p align="center">
  <img src="docs/assets/braid-logo-128.png" width="128" height="128" alt="Braid logo">
</p>

# Braid

Braid turns GitHub Issues and pull requests into durable working memory for
local Coding Agents. GitHub holds the current design, implementation state,
metadata, relationships, and discussion; Braid rebuilds a provider session from
that state whenever prior context becomes stale.

The repository is transitioning from an obsolete Python turn-mirroring
prototype to a clean Rust runtime. The active product and implementation
contracts are:

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
x86_64 follows. The current Python/PDM commands describe withdrawn behavior and
must not be used as product evidence.

Braid deliberately avoids a large internal fake/unit-test surface while the
workflow is being established. Diagnostic and real black-box campaign helpers
belong under [`scripts/tests/`](scripts/tests/), but retained GitHub/provider/
OTel evidence and Human verdicts remain the acceptance oracle.
