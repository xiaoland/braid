# Error Handling and Panic Policy

Read this when defining/changing errors, adding context, deciding between panic and Result, or reviewing retry/exit behavior.

## Recoverable vs bug

Use `Result` for conditions the caller/operator can reasonably encounter at runtime: I/O failure, invalid external input, timeout, conflict, unavailable dependency, missing data, parse/validation failure.

Use panic for programmer errors or violated invariants where continuing would be incorrect and the invariant should have been established before the call.

Do not turn expected malformed user/network input into panic.

## Library errors

A library error type is part of its API when callers branch on it.

Prefer structured variants/source chaining over human-only strings when semantics matter.

Good variants describe categories the caller can act on, not every internal call site.

Avoid exposing a concrete third-party error type as the only public error when that unnecessarily freezes an implementation detail. Preserve the source for diagnostics where useful.

Do not erase to `Box<dyn Error>`/application-style generic errors too early if callers need typed recovery.

## Application errors

At executable/application boundaries, richer contextual errors are useful. If the repository uses an error-context crate, add context such as what operation/resource failed.

Avoid repetitive context:

```text
failed to load config: config load failed: could not load config: ...
```

Each layer should add new information.

Map final errors to stable process exit codes/protocol responses/telemetry at the boundary that knows those semantics.

## `?`

Use `?` when propagation is the right policy. It improves linearity but should not erase necessary translation/context.

Translate an error when crossing a semantic boundary, for example DB unique violation -> `UsernameAlreadyExists`, or parser details -> stable protocol invalid-input error.

Do not map every error to one catch-all variant if callers need to distinguish retryable/not-found/invalid states.

## `unwrap` and `expect`

An `unwrap()`/`expect()` is defensible when failure proves a programmer/build invariant and recovery is not meaningful, for example a constant regex/schema known to be valid.

For `expect`, the message should state the invariant/assumption, not merely "should work".

Avoid unwrap/expect on:

- request/user input;
- filesystem/network/database results;
- channel receives that can close during shutdown;
- optional configuration unless startup validation guarantees it and the invariant is local/obvious;
- locks where poisoning/recovery semantics matter to the application.

Tests/examples may use unwrap for readability when failure should fail the test/example and no production policy is implied.

## Panics across boundaries

Do not let panic cross FFI boundaries unless the ABI/integration explicitly makes that safe; usually catch/prevent it before crossing.

In server/task systems, decide whether a task panic should crash the process, be supervised/restarted, or become an error. Ignoring failed `JoinHandle`s can hide panics.

## Error strings

Human messages should be useful but not become machine protocols accidentally. If callers need stable discrimination, expose typed codes/variants rather than requiring string parsing.

Avoid including secrets, credentials, full sensitive payloads, or raw private data in `Display` because errors often reach logs automatically.

## Retry classification

Retry policy belongs near the operation/application boundary, not inside every low-level helper.

Classify errors into:

- permanent (invalid input, auth, conflict that needs user action);
- transient (timeout, temporary unavailable, selected connection errors);
- ambiguous (request may have succeeded but response was lost).

For ambiguous failures, retry only if the operation is idempotent or protected by an idempotency/deduplication mechanism.

Use bounded attempts/deadlines/backoff. Avoid nested retry loops in multiple layers.

## User-facing/protocol errors

Map internal errors to stable public categories at the protocol boundary. Log/source-chain diagnostics internally; do not expose implementation details merely because `Display` is convenient.
