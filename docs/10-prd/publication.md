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

`braid gh comment create` and `braid gh pr ensure` accept valid local requests
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

Acceptance is not remote authorization: canonical target kind, installation
permissions and Profile compatibility are checked before publication. Source
assignment/Profile revision/session/turn provenance, when available, is retained;
normal turn completion does not discard the obligation, but Context invalidation
and generation replacement fence old output. Existing launch environment flags
are not permission proofs. Source-aware invocation binding remains the extension
boundary for maintenance capabilities; ordinary `gh`, `git`, and shell remain
outside this convenience surface.
