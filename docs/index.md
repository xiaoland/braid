# Braid Documentation

<!-- svc:begin navigation sha256=55c5c21e0087021bc7f81047e2ffe91f889b81829473655d86d78fbe2029d4f2 -->
## SVC CLI

Use `svc --help` and the installed SVC Skills for framework guidance. Project documentation outside this marked block remains Consumer-owned.
<!-- svc:end navigation -->

Read owners progressively from the problem being changed:

- [Product Truth](10-prd/README.md): purpose, promises, workflows, scope
- [End-to-end acceptance](10-prd/acceptance.md): real external oracle
- [Product TDD](20-product-tdd/README.md): authority, Rust topology, durable state
- [GitHub Context](20-product-tdd/context.md): exact Issue/PR memory projection
- [Lifecycle](20-product-tdd/lifecycle.md): events, debounce, invalidation, sessions, reactions
- [Codex app-server](20-product-tdd/app-server.md): provider capability mapping
- [GitHub boundary](20-product-tdd/github.md): ingress, reads, association, writes
- [Deployment](40-deployment/README.md): artifact, config, SQLite migration, OTel, tunnel, operation
- [Glossary](10-prd/glossary.md): canonical vocabulary

Root [`AGENTS.md`](../AGENTS.md) is the actual project instruction entry point.
Task packets are disposable control state and are not linked as durable owners.
