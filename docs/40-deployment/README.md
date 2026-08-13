# Deployment and Operator Contract

This document owns packaged runtime, configuration, migrations, observability,
tunnel supervision, health, and black-box campaign operation. It does not grant
GitHub mutation authority by itself.

## Supported Shape

The first supported artifact is a signed/checksummed macOS arm64 `braid` binary.
It runs without Python, PDM, Cargo, or a source checkout. Linux x86_64 is the
next target. Runtime prerequisites remain explicit:

- Git and `gh` for Agent workflows;
- pinned Codex executable and authenticated provider home;
- Wrangler for the free Quick Tunnel acceptance path;
- GitHub App ID/private key/webhook secret and installation;
- optional OTLP endpoint; full campaign acceptance requires one.

Release artifacts include `braid`, `LICENSE`, `README`, default config example,
OTel Collector example for optional tail sampling, SHA-256 checksums, and
release notes generated from `CHANGELOG.md` under SemVer.

## Configuration

`braid.toml` is versioned independently from binary and database schema. It
contains no secret values; secrets use file references or environment-variable
names. The main sections are:

- GitHub App/repository/handle, API version, webhook and reconciliation;
- Profile definitions/tags/default PR Profile/context byte budgets/status
  surfaces/provider/workspace/resources;
- scheduler quiet/count settings;
- Codex path/version/schema pins and provider home;
- SQLite/runtime/worktree directories;
- loopback ingress/health ports and Wrangler path;
- OTLP endpoint/protocol, trace sample ratio, and incident override;
- log format and bounded health/status settings.

`braid config check` prints the effective non-secret config and all external
preconditions. `braid doctor` additionally runs read-only GitHub, Codex schema,
SQLite directory, Git/worktree, port, OTLP, and Wrangler probes. Profiles expose
their effective mapped provider settings through `braid profile inspect`.

## Filesystem Layout

One private runtime root outside repository worktrees contains:

```text
state/braid.sqlite3
state/backups/
provider/
worktrees/<repository>/<pr-number>-<assignment-generation>/
logs/                         # only when local sampled-log export is configured
```

Issue Profiles use their configured repository checkout/workspace policy. PR
activation provisions one default worktree for the single v1 Implementation
Agent from the selected Development/requested/deterministic branch. Braid
records identity and diagnoses drift but does not intercept or restrict the
Agent's subsequent Git operations.

## Database Lifecycle

`braid migrate plan` reports pending embedded migrations and compatibility.
`braid migrate apply` takes the exclusive migration lease, creates a timestamped
pre-migration backup, verifies historical checksums, and applies transactions.
`braid serve` may apply migrations only when `auto_migrate = true`; otherwise it
refuses with the exact command required.

There are no production down migrations. Release notes declare the oldest
binary compatible with the resulting schema. Rollback across an incompatible
migration restores the stopped pre-migration DB backup into a distinct runtime
directory.

## OpenTelemetry

Braid emits traces, logs, and metrics over OTLP. Metrics are never probabilistic
sampled. Trace roots cover one GitHub delivery/reconciliation change through
Context, scheduling, provider, and Braid-owned GitHub convergence.

The Rust SDK uses parent-based trace-ID ratio head sampling. Default ratio is
`0.10`; acceptance and incident mode set `1.0`. Sampling is decided once at the
root so retained traces do not contain arbitrary middle gaps. Payload-bearing
evidence—including GitHub bodies/summaries, raw webhook, provider transcript,
credentials, API results, and local paths—is attached only to recording spans
as log/event bodies rather than high-cardinality metric labels.

Sampling controls volume, not confidentiality. An OTLP backend receives the
same sensitive material as the runtime and is configured/retained accordingly.
The portable binary does not promise outcome-aware tail sampling. Operators who
need “all errors plus 10% normal” route 100% from Braid to an OpenTelemetry
Collector tail-sampling processor; the Collector remains optional external
infrastructure.

The selected Rust SDK behavior was compiled and measured on 2026-08-13:
parent-based 10% sampling exported 945 of 10,000 independent traces, and every
retained span preserved all five controlled full-payload attributes.

## Startup and Supervision

Startup is ordered:

1. parse/validate config and secrets;
2. acquire process/runtime owner lease;
3. verify/apply permitted DB migrations;
4. probe installed Codex version/schema and GitHub App installation;
5. start OTLP exporters and loopback health/ingress;
6. start Codex adapter and reconcile persisted sessions/turns/outbox;
7. start canonical GitHub reconciliation;
8. optionally start Wrangler, verify a signed public ping through the returned
   URL, then update the GitHub App webhook;
9. mark repository ready for assignment/turn claims.

Supervised task exit changes bounded health and triggers restart/backoff where
safe; provider/tunnel loss does not terminate the whole runtime or invent Agent
terminal state. Shutdown stops new claims, settles/fences active operations,
flushes outbox and OTel within bounded deadlines, restores the prior webhook URL
after a graceful Quick Tunnel stop, releases leases, and closes SQLite.

Quick Tunnel URLs are temporary. After ungraceful death the operator checks and
repairs the App webhook before restart. GitHub does not automatically redeliver
failed webhooks; reconciliation is the recovery path.

## Health and Status

Loopback `/healthz` and `braid status` expose bounded, non-semantic state:

- binary/config/DB/protocol versions;
- owner lease and migration state;
- GitHub/tunnel/reconciliation freshness;
- provider connection and unknown-turn count;
- active/debouncing/blocked group counts;
- oldest pending/uncertain write intent;
- OTel exporter and sampling mode.

Health never reports task acceptance, design readiness, or implementation
success. Operational Status Comments are the GitHub-visible projection for
specific blocked Work Items.

## Real Campaign

Install the candidate artifact into a clean directory, create a private runtime
root, configure the dedicated App/Profiles/OTLP endpoint, and run:

```shell
braid --version
braid config check --config /absolute/path/braid.toml
braid doctor --config /absolute/path/braid.toml
braid migrate plan --config /absolute/path/braid.toml
braid migrate apply --config /absolute/path/braid.toml
braid serve --config /absolute/path/braid.toml
```

Then drive only the real campaign in
[`../10-prd/acceptance.md`](../10-prd/acceptance.md). Source-tree execution,
private DB edits, fake events, or a manually signed local request cannot accept
the release.
