# Acceptance Helpers

This directory contains operator scripts that exercise public boundaries while
the real Braid workflow is being stabilized. A helper may invoke the installed
`braid` binary, GitHub/`gh`, Git/Codex/Wrangler, loopback health, OTLP, and
process controls. It must not import Braid internals, inject SQLite rows/events,
or replace GitHub/provider with fakes and then claim product acceptance.

Scripts should capture bounded machine-readable evidence and leave Human
verdicts explicit. Running a helper is not acceptance: every release candidate
must satisfy [`docs/10-prd/acceptance.md`](../../docs/10-prd/acceptance.md)
through real GitHub Work Items and a clean packaged installation.

`00_clean_install.sh` is the Rust foundation gate. It unpacks the release
artifact, scrubs Python/PDM/Cargo from the binary's `PATH`, exercises only the
public CLI, verifies schema 0→2, schema 1→2 with a pre-v2 backup, and
schema-newer refusal, and uses the adjacent
bounded OTLP/HTTP capture helper to observe sampling. Its direct SQLite write is
limited to constructing declared migration-compatibility fixtures; it does not
inject product events or count as workflow acceptance.

`10_context_projection.sh` is the Slice 1 real-object gate. It requires an
absolute App-backed config, a controlled Issue and PR, plus explicit fixture
expectations for visible/filtered/folded/deleted/paginated evidence and the
number of directly Associated Issues/PRs. It lowers the public diagnostic
GraphQL page size to one, proving the real page walkers without manufacturing
hundreds of comments, and checks that a closed Associated Issue contributes no
body. Missing fixture inputs return
`UNAVAILABLE` rather than substituting a user token, mock server, or synthetic
snapshot. The helper drives only packaged `braid` commands and compares the
emitted bytes.
