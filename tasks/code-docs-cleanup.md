# Code and Documentation Cleanup

- **Goal**: Get the Rust codebase into a maintainable, pre-release shape before any production artifact ships.
- **Scope**:
  1. **Migrations**: Merge the 11 pre-release migrations into a single `0001_initial.sql` migration. No production databases exist yet, so a clean slate is acceptable.
  2. **Repository layout**: Split oversized `src/` files (especially `store.rs`, `runtime.rs`, `context.rs`, `github.rs`, `provider.rs`, `writer.rs`, `cli.rs`) into focused modules aligned with the Product TDD owners. Keep readable deep modules; avoid splitting purely by line count.
  3. **Dependency / duplication audit**: Remove dependencies that are no longer used. Identify places where Braid re-implements a mature crate's happy path (e.g., GitHub App auth, JWT, GraphQL pagination, worktree handling) and replace them where it reduces code without losing the TDD contract.
  4. **Docs**: Trim stale examples, update README/setup docs if the CLI surface changes, and ensure doc comments on public items.
  5. **Verification**: `cargo fmt/check/clippy/test` must pass after each cleanup step. The packaged release must still install via Homebrew and `braid setup --no-browser` must still produce valid output.

## Done

- [x] Migrations collapsed to one init migration.
- [x] Unused dependencies removed from `Cargo.toml`/`Cargo.lock`.
- [x] `provider.rs` refactored into a module tree (`provider/mod.rs`, `provider/codex.rs`, `provider/pi.rs`, `provider/util.rs`) with clean public API.
- [ ] `store.rs` refactored into a module tree (deferred: module cohesion requires keeping `StoreActor`/`Command`/`actor_loop` in `mod.rs`; will extract pure helpers later if needed).
- [x] `runtime.rs` refactored into a module tree (`runtime/mod.rs`, `runtime/ingress.rs`, `runtime/outbox.rs`, `runtime/reconcile.rs`, `runtime/scheduler.rs`, `runtime/issue_agent.rs`, `runtime/pr_agent.rs`, `runtime/provider.rs`, `runtime/tunnel.rs`).
- [ ] `context.rs` refactored into a module tree or typed GraphQL client (deferred: requires AST-aware extraction; manual sed is too risky).
- [ ] `github.rs` reviewed and split if needed (migrated to `octocrab`; kept as single module due to tight identity/REST coupling).
- [ ] `writer.rs`, `cli.rs` reviewed and split if needed (in progress: subagent splitting into handler modules).
- [x] Replace worktree shell commands with `git2`.
- [x] Replace hand-written GitHub App client with `octocrab`.
- [ ] Replace manual GraphQL query building with typed client (deferred: `octocrab` raw `/graphql` is sufficient for current query surface).
- [ ] Replace JSON-RPC-ish provider loop with a typed crate (deferred: split into modules; typed crate not yet adopted).
- [x] Update acceptance test scripts (`00_clean_install.sh`, `30_issue_agent.sh`, `40_context_lifecycle.sh`) for collapsed schema version 1.
- [x] Clean release rebuilt and Homebrew formula updated to v0.2.1.

## Verification

- `cargo fmt/check/clippy/test`
- `brew reinstall xiaoland/braid/braid && braid --version`
- `braid setup xiaoland/braid-poc-test --no-browser` produces the HTML form and manifest JSON

Delete this packet when the cleanup branch is merged into the release line.
