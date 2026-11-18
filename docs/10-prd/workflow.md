## Collaboration Workflow

### Discuss

Issue Activation creates the Issue session. A native assignment and the first
trusted visible `@braid` on a dormant Issue are the same internal `assign`
event; neither invents a turn by itself. On installations without the special
Agent App assignment capability, that first mention both activates the dormant
Issue and supplies the first Wake Event. Later Human comments, newly populated
included metadata, and unfolded content are Wake Events. They accumulate until
the Quiet Window expires or the count threshold is reached. The Issue Agent
receives one current Context plus coalesced Event References and decides
whether to discuss, update the design description, wait, or request
implementation. Relevant external revisions to existing working memory are also
inputs to an already-active responsibility. Replacing an idle session does not
settle that input: Braid supplies the current agreement at the next safe boundary
without requiring another Human mention.

### Implement

An Issue Agent or Human may request implementation through a concise Issue
comment. `braid gh pr ensure` uses that comment's GitHub ID as the
creation idempotency key, so concurrent calls for the same request converge
on one Draft PR. 创建后，原 Implementation Request 与固定的 PR 身份承担长期协作义务，
而不是要求每条催办评论另建一个 PR。后续评论显式复用同一个受管 head 时，只有在
原计划、源 Issue、固定 PR node、base/head、PR Profile 与 canonical 直接关联都一致
且双方仍 OPEN 的条件下才续接原请求。不同 head 的独立请求仍允许形成 1:N。
续接 ensure 不表示新的设计输入已被执行；已激活 PR 的后续处理沿 review Wake／显式
handoff 交付，不隐式增加 turn。 It establishes native Issue association and PR Activation. If
the selected remote head has no difference from base, Braid creates an
App-authored empty bootstrap commit with the same tree, so GitHub can open the
Draft PR before implementation changes exist. This public commit changes no
file and records the Implementation Request; the PR Agent then implements in
the resulting branch/worktree. An applied ensure receipt must confirm durable
delivery of the activation to that exact PR, not merely that the PR exists.
Activation delivery is distinct from executor readiness or implementation
completion; any blocked handover must remain diagnosable.

已固定 PR 的激活交付未完成时，恢复的是原来已授权的职责承诺。Source Context
替换仍禁止旧输出继续远端写入，但不会让已发布 PR 的交接永远依赖旧 turn 存活。
恢复前重验当前 OPEN 状态、精确身份、关联与 Profile；已有匹配的 active/materializing
角色可以幂等采用，已有 blocked/sleeping/retired/finalizing 历史不能被 ensure 自动
解阻或换代。这与“任意关联自动激活 PR”不同。

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
closed-unmerged PR sleeps and a merged PR retires. A mention on a closed Work
Item does not wake the sleeping group; Reopen is the designed re-entry. Reopen
rematerializes Context and starts one ordinary debounced turn. Duplicate
deliveries never grant extra finalization turns.

### Maintaining accepted working memory

Use the source-bound `braid gh pr read` / `issue read --output PATH` snapshot and
`pr edit` / `issue edit --snapshot PATH` workflow described in
[Publication](publication.md#canonical-body-maintenance). Keep accepted Issue
design and PR implementation scope, verification and risks in their respective
bodies. A PR Agent correcting its current directly associated open Issue remains
publicly a PR Implementation Agent. Confirm the returned receipt is applied;
queued design is not yet published. This maintenance action does not create,
activate or schedule another Group. Authorized body text can change GitHub-native associations; only
a subsequent canonical direct-edge confirmation, never submitted body text,
authorizes maintenance of a newly associated open Issue under the existing
source/Context fences. There is no dedicated association, close/merge/ready or
handoff operation added by body maintenance.

### Explicit follow-up to an existing PR

After confirming the Issue design edit receipt is `applied`, its bound Issue
Agent can request `braid gh pr handoff OWNER/REPO#N --request-id STABLE_KEY
--source-turn TURN`. The source Issue must be OPEN; the same-repository PR must
be OPEN/unmerged, GitHub-confirmed directly associated, and already have a current
active schedulable PR assignment. This never creates or reactivates a Group,
unblocks it, reopens it, or enters finalization. Submitted closing text and plain
references are not evidence of a native edge. The Issue body remains the design
source; handoff does not copy a second design authority.
