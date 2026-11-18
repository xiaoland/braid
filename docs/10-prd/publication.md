## Agent Publication and Identity

Braid does not mirror turn activity or final responses. Coding Agents publish
short messages themselves.

## Publication Discretion

A delivered comment, review, or mention never obligates a public reply. The
Agent alone decides what is Human-relevant; a silent turn that only reads,
thinks, or edits local files is a valid outcome. Event References report
changes; they are not commands.

The Agent may keep private working reasoning, drafts, and scratch state as
files in its own worktree — its private, persistent workspace (the SVC task
packet idea). The worktree survives Provider Session replacement within the
same assignment generation, so a fresh session after a Context Reset picks up
where the previous one left off. Braid never publishes private working state.
GitHub receives only Human-relevant conclusions.

## Attribution and Writes

`braid gh` implements the write side needed to use
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

## Durable Publication Receipts

`braid gh comment create`, `braid gh pr ensure`, and body edits accept valid local requests
before connecting to GitHub. Acceptance atomically creates one durable execution
obligation. The command exits with `queued` and a stable public request ID even
when the service is offline or remote validation is rate-limited. The service,
not the CLI process, owns convergence after CLI exit and runtime restart.
Query with `braid gh status <request-ID> --config <path> [--json]`; never resubmit
a body merely to learn its status. Reusing a key with the same request returns
the original receipt; different content conflicts.

Queued/retrying work has not been confirmed published. `uncertain` means a remote
mutation may have happened; Braid reconciles the exact App-authored target/body
and creation window before deciding to resend. Multiple candidates are
`ambiguous`, permanent authorization/target failures are `rejected`, and neither
is retried forever. `applied` for ensure includes native association and initial
Activation, not just PR creation. Internal intent and node IDs are not public
request IDs and are not included in the receipt.

Acceptance is not remote authorization. The public role comes from the source
Group, never the target kind or Profile tags. Tags select eligible roles; they
are not target permissions. Agent mutations require the `--source-turn` reference
supplied in the current runtime prompt. It is only a selector: Braid independently
checks actual cwd, durable worktree, repository, assignment generation, Profile
revision/materials, session, turn and Context fence. `--profile` can only assert
that identity. Removing `BRAID_AGENT` does not enable an implicit human fallback.
An explicit `--operator` entry outside Agent worktrees is audited as Operator;
this is an honest runtime boundary, not an OS sandbox against same-UID attackers.
Ordinary completed turns retain accepted obligations; session/Context/generation
replacement or invalidation forbids further old-source mutations. Recovery can
still confirm a mutation that already happened.

## Canonical Body Maintenance

Issue Agents maintain their own open Issue. PR Implementation Agents maintain
their own open PR title/body and may correct the design of its currently native,
directly associated open Issues. The latter still carries PR Implementation
Agent attribution. Relevant direct cross-surface comments remain allowed; this
does not authorize unrelated targets or add dedicated relationship,
close/merge/ready, or Group-scheduling commands.

Read a canonical snapshot, inspect it, then submit an offline-durable edit:

```shell
braid gh pr read owner/repository#123 --output /tmp/pr-snapshot.json
braid gh pr edit owner/repository#123 --profile pr-codex \
  --source-turn TURN_FROM_RUNTIME_PROMPT --snapshot /tmp/pr-snapshot.json \
  --body-file /tmp/pr-body.md --title 'Current scope' --request-id delivery-update-1
# Use `issue read` / `issue edit` for an Issue. Query the returned request ID.
```

The snapshot carries the canonical version/digest; callers need not guess one.
Title and body are optional independently; empty supplied values and oversized
UTF-8 bytes are rejected. Title-only edits retain the body. Body edits add exactly
one source attribution and retain recognized existing ensure-request metadata and
explicit closing-source Markdown blocks, including ATX and Setext headings.
Protection is of existing data, not words: ordinary “Fixed pagination bug”,
“Resolved review feedback”, code, lists and links are valid replacement content.
Recognized existing sources are preserved as complete original blocks, prefixed
before replacement content so unclosed new fences/comments cannot hide them.
Self-contained top-level paragraphs and headings are supported; an existing
source with links/reference dependencies, HTML or nested/list/code-block context
is explicitly refused when it cannot be safely preserved. Title-only edits can
retain such a body unchanged. This is a bounded source-preservation safeguard,
not a reproduction of GitHub's private parser or proof that arbitrary Markdown
has an unchanged relationship set.

`braid gh` provides identity and attribution, not a Markdown semantic permission
sandbox. An authorized body edit may explicitly add GitHub-native closing text.
This revises the earlier, undelivered keyword freeze and no-relationship-change
restriction. Only GitHub's canonical native direct associations authorize later
cross-surface maintenance: new body text is never itself permission to edit
another Issue. Once GitHub confirms a new edge, that Issue can enter the PR
Agent's maintainable set under the same open-state, source and Context fences.
Ordinary references or closing words alone grant nothing. Body maintenance does
not create, activate or schedule another Group; explicit handoff's direct-edge,
active-target and generation requirements remain separate. The executor
revalidates target state, association and source before mutation and compares the
canonical snapshot again after validation. An external
edit produces a `conflict` receipt, never a blind overwrite. GitHub does not offer
atomic compare-and-swap here: a concurrent edit in the final read/PATCH interval
remains possible; Braid does not simulate CAS with a write-then-rollback.
An `applied` body receipt confirms content, not an invariant relationship set.
After an uncertain send, exact remote title/body is reconciled
before retry; different external content is conflict, not claimed success. No
second execution queue or schema authority is introduced.

### Handoff receipts

`pr handoff TARGET --request-id KEY --source-turn TURN` is Agent-only. Acceptance
is local and durable in the same intent/outbox used for publication. The key is
namespaced by source assignment/generation and binds the first explicit target's
current assignment. Same-key retries return the same receipt; changing target or
request content conflicts. Replacement target generations are never substituted.
An unresolved (`queued`/`sending`/`uncertain`/`ambiguous`) Issue body edit is a clear
refusal boundary: confirm publication first rather than assuming queued design
was published. `braid gh status REQUEST_ID` checks delivery. Handoff `applied`
means only that a Wake was durably delivered to the PR batch, not that design
implementation or acceptance has completed. Preflight rate limits keep the
request pending without creating runnable work.
