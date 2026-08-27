# Runtime / Adapter Registry

## Terminology

- **Adapter**: Braid-internal protocol implementation (e.g. the codex-app-server
  adapter, the pi adapter). It ships inside the Braid binary and is never
  downloaded.
- **Agent Runtime**: the external executable/SDK or HTTP service the adapter
  drives (Codex app-server, Pi, a deepseek-harness endpoint). Runtimes are
  declared in the registry but **never auto-installed**.

## Goal

Define how Braid declares, discovers, and connects to agent runtimes.

## Design Principle

**Never modify the user's machine without explicit authorization.** Braid does
not install runtimes itself. It discovers what exists, prints exact install
commands when nothing is found, and lets the user execute them.

## Agreed Direction

- Profiles reference an adapter by id (e.g., `adapter = "codex-app-server"`).
- Each adapter ships with Braid and provides:
  - a **discovery probe** (find candidate runtimes on the machine, e.g. Codex
    CLI's bundled `codex app-server`, `pi` on PATH/pnpm bin);
  - an **install instruction** (pnpm preferred, npm fallback) printed when
    discovery finds nothing;
  - a **connection verifier** (version/schema handshake; `protocol.rs` already
    does this for Codex).
- Runtime registry entry (per worker, in `config.toml`):
  - `id`, `adapter` (adapter id), `version` (informational pin);
  - connection config — exactly one of:
    - `executable_path` (+ adapter-specific extras like `home`, schema pins);
    - `api_url` (HTTP endpoint, e.g. deepseek-harness);
  - Braid never writes this entry by itself beyond what `braid setup` persists
    after explicit user selection.
- `braid setup` flow: run discovery probes → list candidates → user selects
  one → verify connection → persist. If discovery is empty, print the
  adapter's install command and exit with instructions.
- `braid serve` / `braid doctor` verify the configured runtime is reachable;
  they report, never install.

## Pending Decision

None.
