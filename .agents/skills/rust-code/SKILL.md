---
name: rust-code
description: Design, implement, refactor, debug, and review production Rust applications and libraries, including ownership and borrowing, module/public API design, Result-based error handling, async/concurrency, Tokio or existing runtimes, persistence/integrations, tracing, unsafe/FFI, Cargo features, tests, and workspace tooling. Use for substantive Rust code changes or reviews. Do not use for non-Rust projects or for generated/vendor Rust code unless the task specifically targets it.
compatibility: Rust projects managed by Cargo. Follow the repository's pinned toolchain, edition, MSRV, async runtime, feature model, workspace layout, and CI commands.
metadata:
  version: "1.0.0"
  last-reviewed: "2026-08-27"
---

# Production Rust Engineering

Use this skill as the engineering standard for production Rust. The goal is code whose ownership, failure model, concurrency, public contracts, and unsafe boundaries remain legible to maintainers and enforceable by compiler/tooling where possible.

## Scope gate

Activate for Rust source/Cargo work or when the user explicitly asks to build a Rust application/library.

Do not apply project-specific Rust preferences to generated/vendor code unless editing that code is the task.

## Rule hierarchy

1. Preserve explicit user requirements and externally visible contracts.
2. Preserve the repository's toolchain, MSRV, edition, runtime, workspace, feature policy, and established architecture unless the task changes them.
3. Protect memory safety, correctness, data integrity, security, and compatibility.
4. Apply this skill's defaults to new/touched code when compatible.
5. Avoid unrelated formatting/refactoring churn.

Do not add crates merely because this skill mentions a common library. Prefer the existing dependency stack unless a new dependency solves a concrete problem and is acceptable to the project.

## Before editing

Inspect enough of the actual repository to understand its constraints.

Read, when present:

- root and package `Cargo.toml` files;
- `Cargo.lock` policy and workspace membership;
- `rust-toolchain.toml`, `rust-version`, edition/MSRV declarations;
- `.cargo/config.toml`, Clippy/rustfmt configuration;
- feature definitions and feature combinations used in CI;
- `lib.rs`/`main.rs` and representative module boundaries;
- async runtime and shutdown/task patterns;
- error types and application error/context conventions;
- tracing/logging setup;
- unsafe/FFI modules;
- unit/integration/doc tests and CI commands.

Determine:

- library vs application vs mixed workspace responsibilities;
- public API stability requirements;
- ownership model for long-lived resources;
- sync vs async boundaries and runtime;
- feature compatibility matrix;
- whether panic is acceptable at specific boundaries;
- authoritative fmt/check/clippy/test commands.

Do not assume `--all-features` is valid if features are intentionally mutually exclusive.

## Change workflow

For every non-trivial change:

1. State the behavior/contract and failure modes being changed.
2. Identify the owning crate/module and dependency direction.
3. Choose ownership and error types before layering on clones/boxing/traits.
4. Implement the smallest coherent change.
5. Add tests at the narrowest useful level plus integration/contract tests where boundaries changed.
6. Run formatting, compile/check, Clippy, and tests using the repository's feature/toolchain matrix.
7. Perform the semantic walkthrough; compiler success cannot prove cancellation, retry, API, security, or operational correctness.

## Ownership and borrowing

Let ownership express real lifetime and responsibility.

- Borrow (`&T`, `&mut T`) when the callee only needs temporary access.
- Move values when ownership logically transfers.
- Clone when duplicate ownership/data is part of the intended semantics, not as an automatic response to a borrow-checker error.
- Use `Arc` for genuinely shared ownership across lifetimes/tasks/threads, not as the default smart pointer.
- Introduce interior mutability only when mutation through shared ownership is truly required.
- Prefer small owned values at asynchronous/task boundaries when this simplifies lifetime safety without excessive copying.

Before adding `.clone()`, ask what new owner is being created and how long it should live.

Read [references/architecture-api.md](references/architecture-api.md) when changing modules, traits, ownership boundaries, or public APIs.

## Types and invariants

Use the type system to separate meanings that should not be mixed.

Newtypes/enums are useful for identifiers, units, validated states, and state machines when primitive aliases allow real mistakes.

Do not create wrapper types that add no invariant/semantic/API benefit.

Prefer exhaustive modeling for meaningful states. Use `Option<T>` for genuine absence, not to defer initialization/validation without a reason.

Keep invalid states unrepresentable where doing so simplifies the system; do not over-engineer type-level machinery that obscures straightforward logic.

## Module and public API design

Keep visibility as narrow as practical. `pub` is a compatibility/maintenance commitment, not a convenience to bypass module boundaries.

For public libraries/APIs:

- expose semantic types, not implementation details accidentally;
- document invariants, errors, panics, and safety requirements where material;
- consider forward compatibility before exposing enums/struct fields that consumers must exhaustively match/construct;
- avoid leaking concrete dependency types unless they are intentionally part of the contract.

Do not add a trait merely for mockability. Traits are useful when there are multiple implementations, a consumer-owned abstraction, a plugin/runtime boundary, dynamic dispatch requirement, or meaningful generic contract.

Choose generics vs `dyn Trait` from performance/object-safety/code-size/ABI/ownership needs, not ideology.

## Error model

Use `Result<T, E>` for recoverable failures and panic for bugs/unrecoverable violated invariants, consistent with Rust's model.

Expected operational failures should not become panics because handling them is inconvenient.

- Preserve structured/typed errors at library/domain boundaries when callers need to reason about them.
- Application binaries may add rich context/erasure at outer boundaries when the existing stack supports it.
- Add context where it tells the operator what operation failed, without duplicating the same message at every layer.
- Do not match error strings when a typed variant/source is available.
- Do not expose secrets/sensitive payloads through `Display`, logs, or user-facing errors.

`unwrap()`/`expect()` are acceptable only where the invariant is obvious and failure means a programmer/build-time invariant was violated. In production fallible paths, propagate or handle errors.

Read [references/errors.md](references/errors.md) for library/application error boundaries, panic policy, context, and retry classification.

## Async and concurrency

Preserve the project's runtime. Do not introduce Tokio into a runtime-neutral library or a second runtime into an existing application without a concrete architectural reason.

In async code:

- do not execute blocking I/O or long CPU work on executor threads;
- use the runtime's blocking isolation mechanism for unavoidable blocking work;
- do not hold a synchronous mutex guard across `.await`;
- use an async mutex only when data truly must remain locked across awaits; it is not an automatic upgrade;
- prefer message passing/owned task patterns for asynchronously managed I/O resources when that clarifies ownership;
- bound queues and concurrency;
- define cancellation and shutdown behavior;
- keep/track spawned task handles when task completion/failure matters.

A detached task with no owner is an operational resource leak waiting to happen.

Read [references/async-concurrency.md](references/async-concurrency.md) for Tokio-aware blocking, locks, channels, task ownership, cancellation, timeouts, and graceful shutdown.

## Persistence and external integrations

Keep protocol/database-specific code at adapter boundaries. Define:

- resource/pool lifecycle;
- timeout behavior;
- transaction/atomicity ownership;
- retry classification and idempotency;
- data validation/decoding;
- backpressure/concurrency limits;
- error translation;
- telemetry with sensitive fields excluded.

Do not hide commits, network retries, or infinite timeouts behind innocent-looking helpers.

For async applications, confirm each DB/client library is compatible with the runtime and does not block executor threads.

## Unsafe and FFI

Prefer safe Rust. Do not use `unsafe` merely to escape borrow/lifetime constraints that can be modeled safely.

Every unsafe block/function/impl must have a stated invariant:

- what conditions make the operation safe;
- who establishes them;
- why they remain true for the unsafe operation's lifetime.

Keep unsafe surface small and wrap it behind a safe API when possible. Public `unsafe fn` needs clear safety documentation for callers.

At FFI boundaries, validate pointers, lengths, ownership transfer, alignment, lifetime, callback/thread assumptions, and panic behavior across the boundary.

Read [references/unsafe-security-observability.md](references/unsafe-security-observability.md) for unsafe review, sensitive data, parsing/DoS, and tracing.

## Logging, tracing, and observability

Use the project's tracing/logging stack. For async services, structured spans/events are usually more useful than line-oriented prose.

Record stable fields such as operation/resource class/outcome, not secrets or unbounded payloads.

Avoid logging the same propagated error at every layer. Log at the boundary with useful context or when an error is intentionally handled/retried.

When using `tracing` instrumentation, skip/redact sensitive or huge arguments rather than recording entire request objects automatically.

## Cargo features

Features are part of the build/API surface.

Prefer additive features when practical. Avoid code whose meaning changes incompatibly depending on unrelated feature combinations.

If features are mutually exclusive, make that explicit and test the supported matrix. Do not tell CI to use `--all-features` if that combination is invalid by design.

Optional dependencies exposed as public types can accidentally force downstream feature coupling; review public API under each supported feature set.

## Testing

Use the appropriate level:

- unit tests for local invariants/algorithms;
- integration tests in `tests/` for public crate/application boundaries;
- doc tests for public API examples that should compile/run;
- property/fuzz tests when parsers/protocols/invariants benefit and the project supports them;
- concurrency/failure tests for races, cancellation, timeout, and partial failure where those are the risk.

Test errors and edge cases, not only happy paths.

Avoid sleeping arbitrary wall-clock durations to synchronize async tests. Use controlled time, channels/barriers, deterministic fakes, or runtime test utilities where possible.

Read [references/testing.md](references/testing.md) for feature-matrix, async, property, integration, and regression testing.

## Mechanical gates

Use repository/CI commands first. A typical Cargo project should cover the equivalent of:

```bash
cargo fmt --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Add the repository's supported feature flags/matrix. Use `--all-features` only when all features are designed to coexist.

For public libraries, documentation checks/doc tests may also be part of the gate.

Clippy restriction/pedantic lints should be selected intentionally. Do not enable the entire `clippy::restriction` group; it contains intentionally restrictive and sometimes contradictory lints.

If useful, run the bundled source audit:

```bash
python scripts/audit.py src
```

Warnings are advisory unless `--strict` is used.

Read [references/tooling.md](references/tooling.md) for command and feature-matrix selection.

## Mandatory semantic walkthrough

After automated checks pass, review the diff and answer all applicable questions:

- Is each owned value/resource owned for a clear reason, or did clones/`Arc` hide a lifetime/design problem?
- Is visibility/API surface narrower than necessary or wider than justified?
- Can every operational failure be handled without panic, and are true invariants documented where panic remains?
- Are error variants/context stable and useful without leaking sensitive data?
- Does any async task block the executor, hold the wrong lock across await, or spawn work with no lifecycle owner?
- Are queues/fan-out/retries bounded, and is cancellation/shutdown defined?
- Could partial failure duplicate side effects or violate transaction invariants?
- Does unsafe code have a complete, locally checkable safety argument?
- Are feature combinations and MSRV/public API implications tested?
- Are logs/spans useful without high-cardinality/sensitive fields?
- Do tests force the highest-risk failure/race ordering rather than only the happy path?
- Did a new trait/generic/wrapper improve a real boundary, or add abstraction without payoff?

Do not mark completion solely because the compiler and Clippy are clean.

## Review output

When reviewing, report findings in descending severity. Explain the concrete failure mode (panic, deadlock, race, API break, data loss, unsoundness, leak, etc.), location, and smallest reasonable fix.

Treat unsafe/soundness, concurrency, and data-integrity findings as higher priority than preference-only style comments.

If no material finding exists, say so and note important unverified areas.

## References

- [Architecture, ownership, and public API](references/architecture-api.md)
- [Error handling and panic policy](references/errors.md)
- [Async and concurrency](references/async-concurrency.md)
- [Testing strategy](references/testing.md)
- [Unsafe, security, and observability](references/unsafe-security-observability.md)
- [Cargo tooling and verification](references/tooling.md)
