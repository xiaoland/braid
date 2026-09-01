## Product Objects

### Work Items and Context

- A **GitHub Issue** becomes an Issue Work Item through an Issue Activation.
  A provisioned GitHub Agent App assignment is the preferred native signal;
  an ordinary GitHub App is not a standard assignable user, so the PoC also
  accepts the first Trusted Braid Mention on a dormant Issue. Native assignment
  creates a session and remains idle; mention fallback carries the comment as
  a Wake Event and starts the first turn after materialization.
- A **GitHub PR** becomes a PR Work Item through a trusted `@braid` PR comment
  or the ActivationIntent produced by `braid gh pr ensure`.
- **Issue Context** contains the repository-qualified Issue identity, current
  title and description, material metadata and relationships, and its comment
  lifecycle projection.
- **PR Context** first contains the current projections of every directly
  Associated Issue, then the PR's own minimal implementation context. It is
  rebuilt from current state before every PR turn; it is never a creation-time
  snapshot.
- Full Context is Markdown or plain text for the Agent, never a JSON protocol
  dump. Event user messages are short references; the Agent can use `gh` to
  inspect the changed object.

The exact projection is the [Context contract](../20-product-tdd/context.md).

### Agent Profiles and Groups

An Agent Profile is a versioned Braid configuration containing a provider,
model, reasoning setting, Profile User Instructions, cwd/workspace policy,
sandbox/approval settings, and optional tools, skills, MCP, or other
provider-specific resources. Tags declare whether it can serve `issue`, `pr`,
or both. The Profile `workspace` names a clean source checkout, never the
Agent's cwd: every Agent Group session runs in a dedicated generation-scoped
Braid worktree (the Issue's sole Development branch when unambiguous,
otherwise the default branch; the PR head for a PR Agent).

Braid adds its own versioned System Prompt when materializing a Provider
Session. It explains GitHub Working Memory, Braid and `braid gh`, concise public
comments, and the Issue- or PR-specific role. GitHub Context remains delimited
working data rather than a system instruction.

The architecture can represent multiple parallel Agents without primary or
sub-agent roles. MVP acceptance deliberately uses:

- one active Issue Agent per Issue Agent Group;
- one Implementation Agent per PR Agent Group;
- one dedicated generation-scoped worktree per Agent Group session.

Multi-Agent fan-out is not rejected, but cross-peer ordering, semantic merge,
arbitration, and convergence are outside the MVP correctness claim.
