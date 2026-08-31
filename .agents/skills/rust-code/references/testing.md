# Testing Strategy

Read this when adding tests, fixing concurrency/failure bugs, changing public APIs/features, or creating test infrastructure.

## Unit tests

Put unit tests near implementation for private/local invariants where access to internals is useful.

Test behavior and edge cases rather than every line. Table-driven loops/cases are useful when failure output remains clear.

## Integration tests

Use `tests/` for behavior through the crate's public interface or application boundary.

Integration tests are especially useful for:

- public API compatibility;
- CLI behavior;
- server/protocol boundary;
- filesystem/database integration;
- multi-module workflows.

Do not expose private functions as `pub` solely so integration tests can call them; test through the intended boundary or use unit tests.

## Doc tests

Public documentation examples that users are expected to copy should compile. Cargo's default `cargo test` runs library doc tests unless configured otherwise.

Use `no_run`/`ignore` deliberately and minimally. An ignored example silently rots if there is no other compile check.

## Error-path tests

Test recoverable failure semantics:

- invalid/malformed input;
- not-found/conflict;
- I/O/provider failure;
- timeout/cancellation;
- closed channels/shutdown;
- partial persistence failure;
- duplicate/retry behavior.

A function returning `Result` deserves tests for important `Err` categories, not only `Ok`.

## Concurrency tests

For race/deadlock-sensitive logic, force the ordering with synchronization primitives, channels, barriers, controlled mocks, or runtime utilities.

Do not use `sleep(Duration::from_millis(50))` as the primary synchronization mechanism; timing tests become flaky across machines.

For async runtime code, use its time-control/test utilities when appropriate and already available.

## Property/fuzz tests

Consider property-based or fuzz testing for parsers, serializers, protocol frames, numeric invariants, untrusted binary/text input, and state transitions with large input space.

Do not add a property/fuzz framework for trivial deterministic code without payoff. If the project already has fuzz targets, extend them when the changed parser/input boundary warrants it.

Useful properties include:

- parse never panics for arbitrary bytes/input;
- serialize -> parse round-trip;
- normalization idempotence;
- state invariant after any operation sequence;
- rejected invalid input does not allocate/work without bound.

## Snapshot/golden tests

Use golden/snapshot files for stable protocol/text/format output where exact representation is the contract.

Avoid huge snapshots that reviewers update blindly. Keep diffs reviewable and pair them with semantic assertions when needed.

## Feature tests

Features can change compiled APIs/behavior. Test supported combinations, for example:

- default features;
- no-default-features when supported;
- individual important optional features;
- all-features only if compatible;
- explicitly supported mutually exclusive combinations in separate jobs.

Do not assume a clean default build proves optional code compiles.

## MSRV/toolchain tests

If the crate declares an MSRV, changes must compile/test on it or CI must explicitly define the policy. Do not casually use a newer standard-library/language feature because the local toolchain supports it.

## Database/external tests

Mocking is useful for application error policy, but it cannot prove SQL, transaction, protocol, TLS, or serialization behavior.

Use real controlled integration environments for the boundary properties that matter.

## Regression tests

For a bug:

1. create a test that reproduces the actual failure/race;
2. confirm it fails for the intended reason;
3. implement the fix;
4. keep the test focused on stable behavior.

## Test-only unwrap

`unwrap`/`expect` in tests can improve clarity when any setup failure should fail the test. Prefer `expect` when an invariant/setup message makes a failure much easier to diagnose.
