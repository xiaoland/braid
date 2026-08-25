## Collaboration Workflow

### Discuss

Issue Activation creates the Issue session. A native assignment does not
invent a turn. On installations without the special Agent App assignment
capability, the first trusted visible `@braid` both activates the dormant Issue
and supplies the first Wake Event. Later Human comments, newly populated
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
