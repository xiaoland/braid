# Product Truth

Braid turns a GitHub Issue or pull request into the durable working memory of a
local Coding Agent. GitHub holds the current collaboration state; provider
sessions are replaceable execution contexts built from that state.

The project vocabulary is owned by [`../../glossary.md`](../../glossary.md).
This document owns product purpose, promises, workflows, and exclusions. The
cross-unit realization is owned by [`../20-product-tdd/`](../20-product-tdd/),
and only the real campaign in [`acceptance.md`](acceptance.md) accepts them.

## Purpose and Pressure

Long Coding Agent sessions accumulate tool output, superseded requirements, and
stale operational facts. Provider compaction helps with token pressure but does
not know which GitHub edits, hidden comments, resolved review threads, or
current metadata supersede that history. Traditional chat also leaves settled
design and implementation state scattered across a private transcript.

Braid makes the collaboration surface itself the compacted memory:

- an Issue description carries the current design;
- a PR body carries the current implementation intent and state;
- visible comments retain discussion and Agent messages;
- folded or resolved discussion keeps identity and lifecycle metadata but not
  body content;
- current GitHub metadata and relationships remain available without expanding
  every related object recursively.

An Agent may therefore replace a stale provider session without losing the
team's current understanding. Humans can correct that memory with ordinary
GitHub edits instead of manipulating a private Agent transcript.

## Claims and Evidence

| Product claim | Observable success | Acceptance evidence |
| --- | --- | --- |
| GitHub is the Agent's durable working memory. | A fresh provider session receives the complete current Issue or PR Context and behaves according to edits, folds, deletion, and metadata changes rather than stale history. | Captured rendered Context, GitHub lifecycle state, and the Agent's subsequent public behavior. |
| Discussion and implementation have distinct roles. | Issue Agents discuss and maintain design; PR Implementation Agents work from the PR plus every directly Associated Issue. | Distinct Profiles/sessions, native associations, dedicated PR worktree, comments, and diff. |
| Collaboration is asynchronous by default. | Ordinary activity is debounced and coalesced; only a trusted visible `@braid` gives request-like turn reactions and bypasses debounce. | Event/reaction timestamps and absence of terminal reactions on ordinary batches. |
| GitHub edits can invalidate stale provider context. | Replacement/removal of already-materialized facts fences the old Context Revision and safely replaces the physical session without reviving folded content. | Provider-session generation, canonical Context, interruption, and subsequent Agent behavior. |
| Agents remain autonomous GitHub participants. | Agents publish concise comments and maintain descriptions/metadata themselves. `braid gh` provides stable App-authored writes without preventing normal `gh`, `git`, or shell use. | GitHub authorship, comment/body history, and public command results. |
| The runtime is diagnosable and distributable. | A packaged macOS arm64 binary runs the real flow with durable migrations and sampled full-fidelity OpenTelemetry. | Clean-install campaign, schema ledger, OTLP evidence, and restart/upgrade observations. |

## Product Objects

### Work Items and Context

- A **GitHub Issue** becomes an Issue Work Item when the Braid Agent App is
  assigned. Assignment creates a new Issue Agent Group and physical Provider
  Session but does not by itself fabricate a Human message or start a turn.
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
or both.

Braid adds its own versioned System Prompt when materializing a Provider
Session. It explains GitHub Working Memory, Braid and `braid gh`, concise public
comments, and the Issue- or PR-specific role. GitHub Context remains delimited
working data rather than a system instruction.

The architecture can represent multiple parallel Agents without primary or
sub-agent roles. MVP acceptance deliberately uses:

- one active Issue Agent per Issue Agent Group;
- one Implementation Agent per PR Agent Group;
- one dedicated worktree provisioned for that PR Implementation Agent.

Multi-Agent fan-out is not rejected, but cross-peer ordering, semantic merge,
arbitration, and convergence are outside the MVP correctness claim.

## Collaboration Workflow

### Discuss

Assigning Braid creates the Issue session. New Human comments, newly populated
included metadata, and unfolded content are Wake Events. They accumulate until
the Quiet Window expires or the count threshold is reached. The Issue Agent
receives one current Context plus coalesced Event References and decides
whether to discuss, update the design description, wait, or request
implementation.

### Implement

An Issue Agent or Human may request implementation through a concise Issue
comment. `braid gh pr ensure` uses that comment's GitHub ID as the
Implementation Request key, so concurrent calls for the same request converge
on one Draft PR. It establishes native Issue association and PR Activation. If
the selected remote head has no difference from base, Braid creates an
App-authored empty bootstrap commit with the same tree, so GitHub can open the
Draft PR before implementation changes exist. This public commit changes no
file and records the Implementation Request; the PR Agent then implements in
the resulting branch/worktree.

PR Profile selection is deterministic: use the sole eligible `pr` Profile, or
the configured default when several exist; otherwise leave activation visibly
blocked. The PR session receives all directly Associated Issue Contexts and the
current PR Context. Local Git facts such as head SHA, commits, changed-file
summaries, checks, and normally reviewers stay out of Context because the Agent
can discover them without harm; GitHub changes to those facts arrive as Event
References when available.

### Review and Memory Maintenance

PR comments, reviews, diff comments, and unresolved review threads form the PR
discussion memory. A PR Agent may update a directly Associated Issue when
implementation reveals a design correction. Its own write is included in
future Context but does not wake or reset the same Agent.

Only open Associated Issues contribute full Context. Every closed Issue,
including completed, not-planned, and duplicate Issues, contributes only its
reference, state/reason, and relationship metadata. Reopening restores full
Context on the next materialization.

### Close, Merge, Reopen, and Unassign

Issue unassignment is debounced; once settled it retires the active Issue Agent
Group. Closing an Issue, closing a PR, or merging a PR does not interrupt a
current turn. It grants at most one Finalization Turn, then a closed Issue or
closed-unmerged PR sleeps and a merged PR retires. Reopen rematerializes Context
and starts one ordinary debounced turn. Duplicate deliveries never grant extra
finalization turns.

## Scheduling, Invalidation, and Reactions

Quiet time is debounce, not readiness. The default is 30 seconds and the
default count threshold is eight Wake Events; either condition releases the
batch. A repository MAINTAIN/ADMIN actor's exact visible `@braid` bypasses both.
Code, quotes, HTML comments, Braid-origin content, and less-privileged actors do
not create a trusted mention.

Replacing or removing a fact already present in a Work Item's Context is Hard
Invalidation. Braid fences the stale Context Revision immediately. If a turn is
active, it requests safe interruption, discards later stale Agent output, starts
a fresh physical Provider Session with current Context, and continues once
with the invalidation reference. If idle, it replaces Context without starting
a turn. Rapid Cross-surface edits to an open Associated Issue description wait
for debounce before interrupting a PR turn. Other Associated Issue changes mark
the dependency dirty and are incorporated before the next PR turn.

Every newly ingested external comment receives Braid's `eyes` reaction. Only a
Trusted Braid Mention has turn-lifecycle reactions on that same comment:

- `rocket` after the provider accepts the turn;
- `+1` after a normal terminal;
- `confused` after a confirmed unexpected terminal;
- back to `eyes` after safe invalidation supersession;
- `eyes` plus `rocket` while the result remains unknown.

Ordinary debounced turns never receive active or terminal reactions. Their
operational failures use one mutable Operational Status Comment instead, so
the normal collaboration model does not look like a request/response pipeline.

## Agent Publication and Identity

Braid does not mirror turn activity or final responses. Coding Agents publish
short messages themselves. `braid gh` implements the write side needed to use
the stable Braid App identity and prepends an immutable attribution block:

```markdown
> **Braid Agent · profile-display-name**
> PR Implementation Agent

The concise Agent message starts here.
```

The role is `Issue Agent` or `PR Implementation Agent`; provider/model/internal
IDs remain absent. `braid gh` is a convenience and identity surface, not a
permission sandbox. The Agent may still use its ordinary `gh`, `git`, and shell
capabilities. Correlated Braid App writes and writes made by a Profile's
explicitly configured stable GitHub actor are Agent-origin. An uncorrelated
write from any other identity is treated as an external GitHub change and may
therefore wake or invalidate the Agent.

## Context Pressure

Each Profile sets a maximum complete GitHub Context byte budget and a soft
ratio, default 80 percent. The byte budget is explicit because providers do not
expose one reliable tokenizer/window contract across models. Soft pressure asks
the Agent or Human to update descriptions, shorten Agent-owned comments, or
fold obsolete discussion. Exceeding the hard budget, or failing to paginate a
required connection completely, blocks the turn and updates an Operational
Status Comment. Braid never silently truncates or summarizes canonical Context.

## Scope and Exclusions

The MVP includes Codex app-server, macOS arm64, real GitHub, SQLite, a free
Cloudflare tunnel for acceptance, `braid gh` writes, packaged installation,
OpenTelemetry, and the Issue-to-PR workflow above. Linux x86_64 follows with
the same contract.

The MVP does not promise Pi/Claude adapters, hosted ingress, deterministic
multi-Agent merge, automatic merge, universal GitHub CLI parity, automatic
semantic readiness/acceptance, or recovery of a provider transcript that the
provider itself has lost.
