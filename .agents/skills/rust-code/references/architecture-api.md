# Architecture, Ownership, and Public API

Read this when changing crate/module boundaries, traits, ownership, public types, or application composition.

## Crates are architectural boundaries

Do not split into crates solely for folder organization. A crate boundary is useful when it provides:

- an independently reusable/stable library API;
- dependency isolation;
- compile-time/feature separation;
- plugin/FFI boundary;
- meaningful ownership/team/deployment boundary.

Too many tiny crates increase compile/config/API overhead. Too few can blur dependency direction. Follow the repository's existing workspace philosophy.

## Dependency direction

For a service/application, a useful conceptual direction is:

```text
binary/composition -> adapters -> application/core
```

The core should not need CLI/HTTP/database/runtime-specific types merely because composition is convenient.

Rust does not require class-style "ports and adapters" ceremony. A function parameter, generic, closure, or small trait may be enough.

## Ownership questions

Before fixing a borrow error, answer:

- Who logically owns this value?
- Is the callee reading, mutating, consuming, storing, or sharing it?
- Must it outlive the call/task?
- Is duplication semantically correct and cheap enough?

Then choose borrow/move/clone/`Arc` accordingly.

A `.clone()` that creates an independent owner of an `Arc`, sender, handle, or cheap immutable value may be exactly right. The issue is unexplained ownership, not clone count.

## Borrowing APIs

Accept borrowed forms when ownership is unnecessary and it improves caller flexibility. Examples often include `&str`, `&Path`, slices, or references to domain types.

Do not mechanically replace every owned parameter with a borrow. Async tasks, storage, thread boundaries, and transformation pipelines often need ownership.

Avoid needless `&String`/`&Vec<T>` when `&str`/`&[T]` expresses the contract and improves interoperability, unless API compatibility says otherwise.

## Newtypes

Use newtypes for distinctions the compiler should protect:

```text
UserId vs OrderId
Milliseconds vs Seconds
ValidatedEmail vs String
```

Keep conversion/validation APIs clear. Avoid dozens of wrappers when values are purely local and confusion is implausible.

## Traits

Introduce a trait when it expresses a real abstraction:

- multiple implementations;
- consumer-side dependency boundary;
- plugin/dynamic dispatch;
- generic algorithm contract;
- platform-specific behavior.

Do not create `FooService` trait + `FooServiceImpl` for every concrete type merely to mimic another language's architecture.

Traits used only for tests can be justified when the boundary is expensive/nondeterministic, but first consider injecting a function/closure/concrete fake or testing at the real boundary.

## Generics vs trait objects

Generics provide static dispatch and type specialization but can increase compile time/code size and spread generic parameters through APIs.

Trait objects provide runtime polymorphism/heterogeneous storage but require object safety and an indirection/vtable.

Choose based on boundary needs. Do not convert one to the other solely for style.

## Public visibility

Default private. Escalate intentionally:

- `pub(super)`/`pub(crate)` for internal collaboration;
- `pub` when downstream consumers should rely on it.

A public function/type is harder to rename, reshape, or remove. Search downstream workspace uses and consider external users before public API changes.

## Public structs/enums

Public fields let downstream code construct and destructure the representation directly. Prefer constructors/accessors when invariants/evolution need control.

For public enums expected to evolve across crate versions, evaluate whether consumers should be forced to handle every future variant. Use the project's compatibility policy; do not add `#[non_exhaustive]` mechanically to all enums.

## Dependency types in APIs

Exposing a third-party type can be correct when that type is intentionally part of interoperability. Otherwise it couples downstream users to your dependency/version/feature choices.

For stable library boundaries, consider owned semantic types or narrow traits rather than leaking DB/runtime/client implementation types.

## Application composition

Keep wiring near binary/application startup. Construct config, pools/clients, services, task supervision, and routes/commands there rather than in module-level global mutable state.

Use `OnceLock`/lazy globals only for values with genuinely process-global immutable/synchronized semantics and clear test behavior.

## Abstraction review smells

Investigate:

- `Arc<Mutex<...>>` added to resolve every ownership issue;
- traits with exactly one implementation and no boundary benefit;
- all functions made `pub` to solve module access;
- large "context" structs passed everywhere with unrelated dependencies;
- generic parameters propagating through many layers for one swappable adapter;
- conversion chains among near-identical wrapper structs;
- `Box<dyn Error>` deep in a library where callers need typed handling.
