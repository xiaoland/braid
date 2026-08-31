# Unsafe, Security, and Observability

Read this for unsafe/FFI, untrusted input, secrets, subprocess/filesystem/network boundaries, and tracing/logging.

## Unsafe review

An unsafe block is a proof obligation.

For each unsafe operation, document/verify:

- pointer/reference validity;
- alignment;
- initialized memory;
- aliasing/exclusivity;
- lifetime/outliving assumptions;
- bounds/length arithmetic;
- thread-safety/Send/Sync assumptions;
- ownership transfer and exactly-once destruction.

Place a `// SAFETY:` explanation adjacent to the unsafe block when the project convention permits. The comment should explain why preconditions hold here, not restate the operation.

Public `unsafe fn` should document caller obligations in a `# Safety` section or equivalent project standard.

Keep unsafe inside a small module and expose a safe interface that validates invariants when possible.

## FFI

At FFI boundaries:

- use ABI-compatible types;
- validate null pointers and length relationships;
- establish who allocates/frees memory and with which allocator;
- prevent Rust panics unwinding across a boundary unless explicitly supported;
- define callback thread/lifetime rules;
- convert strings with encoding/NUL rules intentionally.

Do not construct arbitrary slices/strings from foreign pointers before validating lengths and invariants.

## Untrusted parsing

Safe Rust prevents many memory errors but not denial-of-service or logic vulnerabilities.

For untrusted input, bound:

- allocation sizes;
- recursion/nesting;
- collection counts;
- decompression ratios;
- regex/backtracking-like work in libraries that can exhibit it;
- integer conversions/arithmetic;
- processing time where operationally possible.

Avoid `read_to_end`/collecting an unbounded stream from an untrusted peer without a limit.

## Integer and index safety

Do not use wrapping/truncating casts for external sizes/IDs unless wrap/truncation is the explicit protocol semantics.

Use checked/try conversions where overflow changes correctness/security.

Remember release arithmetic overflow behavior can differ from debug for ordinary integer operators depending on configuration; do not rely on debug panics as validation.

## Filesystem

When user input influences paths, define whether traversal/symlinks/absolute paths are allowed. Joining a base directory with an untrusted path does not automatically confine access.

For security-sensitive creation/update, consider atomic write/rename, permissions, temp-file ownership, and symlink races according to platform requirements.

## Subprocesses

Prefer direct argv execution over invoking a shell. If shell is required, treat interpolation as an injection boundary.

Set timeouts/cancellation where long-running subprocesses can wedge the application, and decide how child processes are terminated/reaped on shutdown.

## Network clients

Use bounded timeouts, TLS verification according to trust requirements, response/body limits, and retry policies safe for the operation.

If URLs/hosts are user-controlled, consider SSRF/private-address/redirect behavior.

## Secrets

Do not log secrets or include them in error `Display`/`Debug` accidentally.

Use secret-wrapper/redaction crates already adopted by the project when available. Do not derive `Debug` on secret-bearing structs without reviewing output.

Zeroization is needed only when the threat model requires reducing memory remnants; use a vetted library rather than handwritten volatile tricks.

## Tracing

Structured tracing is especially useful for async systems because tasks interleave on threads.

Prefer spans around meaningful operations and fields such as request/job/resource class, not concatenated message strings.

Avoid high-cardinality/sensitive fields. For `#[instrument]`, use `skip`/manual fields for large or secret arguments.

Record an error at the boundary that can add useful context or where it is handled/retried. Repeatedly logging the same propagated error makes incidents harder to read.

## Metrics

Bound label cardinality. User IDs, full URLs, error strings, filenames, and request IDs normally do not belong in metric labels.

Useful signals include operation latency/error classes, queue depth, retry count, connection/pool saturation, and task outcomes.
