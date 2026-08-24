# Code and Documentation Cleanup

- **Goal**: Get the Rust codebase into a maintainable, pre-release shape before any production artifact ships.
- **Scope**:
  1. **Migrations**: Merge the 11 pre-release migrations into a single `0001_initial.sql` migration. No production databases exist yet, so a clean slate is acceptable.
  2. **Repository layout**: Split oversized `src/` files (especially `store.rs`, `runtime.rs`, `context.rs`, `github.rs`, `provider.rs`, `writer.rs`, `cli.rs`) into focused modules aligned with the Product TDD owners. Keep readable deep modules; avoid splitting purely by line count.
  3. **Dependency / duplication audit**: Remove dependencies that are no longer used. Identify places where Braid re-implements a mature crate's happy path (e.g., GitHub App auth, JWT, GraphQL pagination, worktree handling) and replace them where it reduces code without losing the TDD contract.
  4. **Docs**: Trim stale examples, update README/setup docs if the CLI surface changes, and ensure doc comments on public items.
  5. **Verification**: `cargo fmt/check/clippy/test` must pass after each cleanup step. The packaged release must still install via Homebrew and `braid setup --no-browser` must still produce valid output.

## Done

- [ ] Migrations collapsed to one init migration.
- [ ] Unused dependencies removed from `Cargo.toml`/`Cargo.lock`.
- [ ] `store.rs` refactored into a module tree.
- [ ] `runtime.rs` refactored into a module tree.
- [ ] `context.rs` refactored into a module tree.
- [ ] `github.rs`, `provider.rs`, `writer.rs`, `cli.rs` reviewed and split if needed.
- [ ] Duplicate/utility code replaced with mature crates where appropriate.
- [ ] Docs updated.
- [ ] Clean release rebuilt and Homebrew formula updated.

## Verification

- `cargo fmt/check/clippy/test`
- `brew reinstall xiaoland/braid/braid && braid --version`
- `braid setup xiaoland/braid-poc-test --no-browser` produces the HTML form and manifest JSON

Delete this packet when the cleanup branch is merged into the release line.
