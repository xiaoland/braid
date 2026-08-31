# Cargo Tooling and Verification

Read this when choosing build/test/lint commands, changing features/toolchain, or setting CI gates.

## Toolchain first

Honor, in order, the repository's:

- `rust-toolchain.toml`/`rust-toolchain`;
- package `rust-version`/MSRV policy;
- CI toolchain matrix;
- edition.

Do not update the toolchain or MSRV as collateral work unless required/approved.

## Formatting

Use the repository's configured rustfmt/toolchain:

```bash
cargo fmt --check
```

For workspaces with unusual config, use the project's wrapper command.

## Compile/check

`cargo check` is a fast compile/type/borrow validation gate. For workspaces/applications, include the targets and feature combinations used in CI.

Common shape:

```bash
cargo check --workspace --all-targets
```

Do not assume examples/benches compile if the gate excludes them intentionally; follow project policy.

## Clippy

Clippy's default groups are designed for broad use. CI can elevate warnings to errors:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Add features according to the supported matrix.

Do not enable the entire `clippy::restriction` group. It intentionally restricts language features and contains lints that may conflict. Cherry-pick restriction lints the project agrees to enforce.

`pedantic` is also opinionated; use the repository's configured selection rather than forcing it during a feature task.

Narrow `#[allow]` is acceptable when the lint is wrong for a specific case. Keep the exception local and explain it if the reason is not obvious.

## Tests

Default:

```bash
cargo test --workspace
```

Cargo also runs library doc tests by default. For feature-heavy projects, test the supported feature matrix explicitly.

Use `--no-fail-fast` in CI only when collecting all package failures is useful; local iteration may prefer fast failure.

If the project uses nextest or another runner, preserve it.

## Features

Before using `--all-features`, inspect `Cargo.toml` and CI. Some projects intentionally define mutually exclusive backends/runtimes.

A robust matrix may include:

```text
default
--no-default-features
--features backend-a
--features backend-b
```

rather than one invalid all-features build.

## Documentation/public APIs

For reusable libraries, consider the project's doc gate, e.g. `cargo doc --no-deps`, doc tests, or rustdoc lints.

Public API/semver analysis tools can be valuable when already used; do not add them without need.

## Security/dependency tools

Use existing project tools such as advisory/license/dependency audits if configured. Do not assume a clean Clippy run audits dependency vulnerabilities/licenses.

## `scripts/audit.py`

The bundled audit is intentionally small and source-oriented. It checks a few high-signal patterns that compiler/Clippy configuration may not enforce uniformly.

Run:

```bash
python scripts/audit.py src crates/foo/src
```

By default:

- errors fail;
- warnings are advisory;
- `--strict` makes warnings fail.

It is a lexical/structural helper, not a Rust parser. Treat unusual macro/generated code carefully and prefer Clippy/compiler diagnostics when they understand semantics.

## Suggested gate order

1. `cargo fmt --check`;
2. `cargo check` with relevant workspace/features/targets;
3. `cargo clippy` with the same relevant matrix;
4. focused tests;
5. full supported tests/doc tests;
6. integration/security/build packaging gates from CI.

## Do not over-mechanize

Keep these semantic:

- whether clone/Arc/trait abstractions are justified;
- panic vs Result policy at a particular invariant;
- task lifecycle/cancellation correctness;
- transaction/retry/idempotency correctness;
- unsafe proof validity;
- public API/semver implications;
- observability usefulness/sensitivity.
