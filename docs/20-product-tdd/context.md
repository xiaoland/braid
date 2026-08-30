# GitHub Context Contract

This document owns the deterministic model-to-text projection that makes GitHub
the Agent's working memory. For the product-level description of what belongs
in Issue Context and PR Context, see `docs/10-prd/objects.md` and
`docs/10-prd/workflow.md`. It does not own webhook scheduling or provider
history.

## Materialization Transaction

For one Work Item Braid reads every required GitHub connection to exhaustion,
then checks the root object `updatedAt` and relationship version again. If the
root or association graph changed during the read, it discards the partial
snapshot and retries a bounded number of times. Missing permission, unsupported
union members, pagination failure, or repeated drift produces
`context-unavailable`; Braid never emits a partial Context.

GitHub GraphQL node IDs are internal routing identities. The Agent-facing
identity is plain repository-qualified text: `GitHub Issue: owner/repo#123` or
`GitHub PR: owner/repo#456`. It is not duplicated as a URL or JSON field.

## Issue Context

Sections appear in this order; empty/default sections are omitted:

1. `# GitHub Issue: owner/repo#number`
2. title as the next plain line;
3. state and close reason;
4. author, issue type, assignees, labels, milestone;
5. Projects V2 entries and non-empty Human fields;
6. Development linked branches;
7. direct parent, sub-issue, blocking, blocked-by, duplicate/canonical, and
   native associated-PR references;
8. `## Description` with the current Markdown body;
9. `## Comments` in creation order.

Set-valued metadata is sorted by stable Human name, then repository-qualified
identity. Project entries sort by project title and field name. Relationships
are references only and never recursively expand another Issue Context.

An Issue comment renders as:

```markdown
### Comment: owner/repo#issuecomment-123 by @octocat
Posted: 2026-08-13T09:30:00Z
Updated: 2026-08-13T10:00:00Z

Visible Markdown body.
```

The `Updated` line appears only when different from creation. A minimized or
otherwise folded comment renders identity, author, timestamps, and
`State: minimized (<reason>)`, but no body. A deleted comment previously seen by
Braid renders the last known identity/author/time metadata and
`State: deleted`; deleted body is never retained in Context. Pinned state is
material metadata. Reactions are not Context.

## PR Context

A PR Context begins with every directly Associated Issue sorted by repository
name and Issue number:

- an open Issue contributes its complete current Issue Context;
- any closed Issue contributes one reference block containing state, close
  reason, and direct relationship metadata only.

The PR portion then appears in this order:

1. `# GitHub PR: owner/repo#number`
2. title;
3. state and Draft/Ready/Merged state;
4. author, base ref, head repository/ref, assignees, material labels, milestone,
   and Projects V2 fields;
5. `## Description` with current PR body;
6. `## Conversation` with PR Issue comments;
7. `## Reviews` in submission order;
8. `## Review Threads` ordered by creation evidence and canonical location.

Visible reviews and review comments use the same identity/author/time/body
shape as Issue comments. A resolved, collapsed, or minimized review thread
keeps path/line when present, authors, timestamps, resolution and
outdated state, plus the resolver when GitHub exposes one, but omits all
comment bodies. An unresolved visible thread
includes its visible comment bodies. Dismissed reviews retain their canonical
state and visible body unless separately minimized.

Head SHA, commit list/summary, changed-file summary, check results, mergeability,
and normally requested reviewers are deliberately absent. They are cheap and
safer for an implementation Agent to inspect locally or through `gh`; GitHub
changes to them produce Event References when subscribed.

## Body Filtering

Braid preserves original Markdown bytes except for actual HTML comment nodes.
The Context projector parses GitHub Flavored Markdown, obtains source ranges for
HTML inline/block comments beginning with `<!--`, and splices those ranges from
the original UTF-8 body. Text that resembles `<!-- -->` inside fenced or inline
code remains. Malformed HTML that the parser does not recognize as a comment is
ordinary visible text.

Operational Status Comments are excluded by their persisted GitHub comment IDs.
Agent comments remain included. No hidden marker is required.

## Snapshot, Revision, and Version Ordering

The internal snapshot is typed Rust data, not an Agent-facing serialization.
Only versions from the same GitHub object shape are ordered. A review-thread
webhook provides authoritative lifecycle evidence but not the full GraphQL
thread shape, so Braid applies `resolved`/`unresolved`, rereads the canonical
thread, and then normalizes its stored projection. It never compares a webhook
timestamp with a synthesized GraphQL thread version. A late delivery can cause
an extra canonical reread, but cannot become Agent Context without that reread.

`Context Revision` is an internal exact-content fingerprint used only to fence
stale provider output and determine session compatibility. This is one of the
few places where a digest is necessary: GitHub exposes a graph of independently
versioned fields but no atomic version for the complete rendered Context. The
fingerprint is not a product identity and never appears in Agent Context, Event
References, `braid gh` output, or public Context diagnostics. Rendered bytes are
the revision authority; JSON map ordering cannot change it.

## Event References

An Event Reference is one or more direct lines, for example:

```text
GitHub comment owner/repo#issuecomment-123 was created by @octocat.
GitHub PR owner/repo#45 received new commits.
Review thread on GitHub PR owner/repo#45 was resolved.
```

It contains no body, webhook envelope, GraphQL node ID, UUID, digest, internal
generation/revision, provider identifier, or debug JSON. A coalesced turn sorts
references by Braid receipt sequence and removes superseded versions of the
same object.

## Completeness and Pressure

Profiles configure `github_context_hard_bytes` and
`github_context_soft_ratio` (default `0.80`). Braid renders the full Context,
then measures UTF-8 bytes. Above soft pressure it proceeds and exposes status;
above hard pressure it refuses the turn. It does not truncate comments,
relationships, project fields, or Associated Issues, and does not ask a model
to summarize them as a substitute for canonical state.

Provider instructions, tool schemas, Event References, and response reserve are
not included in this byte count; the operator chooses the Profile limit with
those costs in mind. This explicit contract is more honest than pretending all
providers expose the same tokenizer or effective window.

## Context Reset and Resume Compatibility

Resume compatibility is decided by the **group layer**, not the adapter: on
reconnect, the worker compares each persisted provider session against the
effective Profile (repository, profile id and revision, instruction revision)
and either resumes the physical session or blocks it with an Operational
Status update. Context replacement is likewise core-orchestrated: the store
fences the old turn, and the group layer starts a fresh physical session with
the latest materialized context. The adapter carries no revision or digest
state and makes no replace/inject decisions.

This keeps the Profile as plain configuration and avoids coupling the core
runtime to provider-specific compaction or resume rules.
