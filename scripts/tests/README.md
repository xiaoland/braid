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
