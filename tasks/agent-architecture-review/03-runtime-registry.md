# Runtime / Adapter Registry

## Terminology

- **Agent Runtime Adapter**: a Braid-internal protocol implementation class
  (e.g. codex-app-server adapter, pi adapter), shipped inside the Braid binary.
  Identified by `adapter_type` + contract `version`.
- **Agent Runtime**: the external executable/SDK or HTTP service an adapter
  instance connects to (Codex app-server, Pi, a deepseek-harness endpoint).
- **Connectivity config**: adapter-defined parameters used at instantiation to
  connect to/use a runtime — e.g. `executable_path`, `api_url`,
  `CODEX_HOME`/`PI_HOME`, schema pins. Its shape is owned by each adapter,
  not by a universal schema.

## Relationship Model

```
Agent Profile (adapter_type + version)
        │  locates
        ▼
Agent Runtime Adapter class ──instantiated with──▶ connectivity config
        │                                              (registry entry,
        ▼                                               worker-level)
   connects to / uses
        ▼
Agent Runtime
```

- A profile references **only** `adapter_type` + `version`; it never carries
  connectivity parameters.
- Connectivity config (including `CODEX_HOME`/`PI_HOME`-style homes) lives in
  the per-worker registry entry. Rationale: a profile's `user_instructions`,
  `skills`, and `mcps` are implemented against a specific runtime home;
  allowing per-profile homes would make the same profile resolve different
  skill sets and break the role-snapshot abstraction.

## Design Principle

**Never modify the user's machine without explicit authorization.** Braid does
not install runtimes itself. It discovers what exists, prints exact install
commands when nothing is found, and lets the user execute them.

## Registry Entry (per worker, in `config.toml`)

- `adapter_type`, `version` (verified at setup time);
- connectivity config in the adapter's own shape (e.g. Codex:
  `executable_path` + `home` + schema pins; Pi: `executable_path` + `home`,
  or `api_url` for HTTP-serving runtimes such as deepseek-harness);
- one entry per `adapter_type` per worker.

## Adapter Responsibilities (compiled into Braid)

- **Discovery probe**: find candidate runtimes (e.g. Codex CLI's bundled
  `codex app-server`, `pi` on PATH/pnpm bin).
- **Install instruction**: printed when discovery finds nothing (pnpm
  preferred, npm fallback); Braid never executes it.
- **Connection verifier**: version/schema handshake (`protocol.rs` already
  does this for Codex).

## Setup / Serve Flow

- `braid setup`: run discovery probes → list candidates → user selects →
  verify → persist registry entry + default profile. Empty discovery prints
  the install command and exits with instructions. Manual flags
  (`--runtime-executable`, `--runtime-api-url`) bypass discovery.
- `braid serve` / `braid doctor`: verify the configured runtime is reachable
  and contract-compatible; report, never install.

## Pending Decision

None.
