# Async and Concurrency

Read this for Tokio/other runtimes, blocking work, mutexes, channels, spawned tasks, cancellation, timeouts, and shutdown.

## Runtime discipline

Use the project's runtime and its utilities. Libraries that do not need a runtime should remain runtime-neutral where practical.

Do not call `block_on` inside an async runtime path as a shortcut unless the runtime explicitly supports the pattern and architecture requires it.

## Blocking work

Async executors rely on tasks yielding. Blocking filesystem/network/process calls or long CPU loops can stall unrelated tasks.

For Tokio applications, isolate unavoidable blocking operations with its blocking facility (for example `spawn_blocking`) and bound concurrency/resource use.

A blocking-thread pool is not a substitute for a job system for unbounded CPU-heavy work.

`std::thread::sleep` must not be used inside async task logic; use the runtime timer.

## Mutex selection

Do not use an async mutex solely because surrounding code is async.

A synchronous mutex can be appropriate for a short, low-contention critical section that never crosses `.await`.

Never hold a synchronous mutex guard across `.await`. Restructure scope so the guard is dropped first.

Use an async mutex when the protected operation must legitimately remain locked across awaits. Recognize that this serializes access and may reduce throughput.

For an I/O resource needing exclusive async access, a dedicated owner task plus message passing is often clearer and supports pipelining/backpressure better than `Arc<Mutex<Client>>`.

## Channels and backpressure

Prefer bounded channels for producer/consumer pipelines unless an unbounded channel has a proven bounded producer/lifetime.

Define behavior when full/closed:

- wait;
- drop/coalesce;
- reject work;
- terminate.

An unbounded queue converts overload into memory growth.

## Spawned tasks

A spawned task should have an owner/lifecycle model.

For every spawn, answer:

- Who observes `JoinHandle` success/panic/error?
- How is cancellation requested?
- Does shutdown wait for it?
- What resources does it retain?
- Can it outlive the data/request that created it?

Detached telemetry/best-effort tasks can be valid, but make the loss/failure semantics explicit.

## Cancellation

Dropping/cancelling a future can occur at an `.await`. Code that owns partially updated state or external side effects must be cancellation-safe or protected by a protocol/transaction.

Be careful with `select!`: branches can cancel losing futures. Verify whether those operations are safe to cancel and what progress is lost.

For long-lived services, use a clear cancellation signal/token/channel and propagate it through owned tasks.

## Timeouts

Place timeouts at meaningful operation boundaries. A timeout returns control to the caller but may cancel/drop the underlying future; understand side effects and driver behavior.

Do not stack unrelated nested timeouts that produce unpredictable effective deadlines.

Prefer a propagated deadline when multiple sub-operations must fit one request budget.

## Graceful shutdown

A robust async app usually needs:

1. detect shutdown signal/condition;
2. stop accepting new work;
3. signal tasks;
4. allow bounded drain/cleanup;
5. await tracked tasks;
6. close/flush resources.

Do not spawn important background workers and then let `main` exit without waiting for them.

## Shared state

Prefer immutable shared data and message passing where practical. For mutable shared state:

- choose the narrowest lock;
- minimize critical-section duration;
- do not perform slow I/O while locked unless serialization is the design;
- avoid lock-order cycles;
- consider sharding when contention is measured.

## Atomics

Use atomics when the state and memory-ordering contract are simple and understood. Do not replace a mutex with atomics for style/performance speculation.

Use the weakest ordering that is correct only when maintainers can justify it; `SeqCst` may be a reasonable default for low-frequency simple coordination, but performance-critical lock-free code requires careful proof/testing.

## Async traits/lifetimes

Do not box futures or add `'static` bounds mechanically to silence errors. `'static` on a spawned future means it owns/references data that can live for the task's lifetime; it does not mean data literally lives forever.

Move owned handles into spawned tasks when ownership transfer is correct. Avoid leaking memory to obtain a `'static` reference.
