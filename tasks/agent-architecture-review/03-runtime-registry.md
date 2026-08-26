# Runtime / Adapter Registry

## Terminology

- **Adapter**: Braid-internal protocol implementation (e.g. the codex-app-server
  adapter, the pi adapter). It ships inside the Braid binary and is never
  downloaded.
- **Agent Runtime**: the external executable/SDK the adapter drives (Codex
  app-server, Pi). Runtimes are managed by the registry.

## Goal

Define how Braid declares and manages agent runtime executables.

## Agreed Direction

- Profiles reference an adapter/runtime by id (e.g., `adapter = "codex-app-server"`).
- Runtime registry defines:
  - `type`: protocol/adapter identifier
  - `version`: pinned version for contract compatibility
  - `download_url` / checksum / executable path
  - isolated installation directory inside the worker folder so Braid can manage
    the binary without polluting the user environment
- Version pin helps Braid avoid contract drift and enables reproducible installs.

## Decisions

1. Registry scope: per worker folder (see `05-worker-layout.md`). Each worker
   pins its own runtime versions without affecting others.
2. Install trigger: `braid setup` asks the user to pick a default agent runtime,
   creates a default agent profile referencing it, and installs that runtime on
   demand. `braid serve` verifies pinned runtimes are present and installs any
   missing ones before starting. A dedicated `braid runtime install` command can
   be added later.
3. Adapter capabilities are declared by the adapter implementation itself
   (compiled into Braid), not by the runtime registry. Profiles do not configure
   capabilities; they only reference the adapter id.

## Pending Decision

None; implement registry schema and setup/serve integration in code.
