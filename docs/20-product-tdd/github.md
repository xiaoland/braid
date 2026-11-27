# GitHub Boundary Contract

This document owns the technical boundary between Braid and GitHub: App
identity, permissions, installation scope, webhook/GraphQL/REST contracts, and
write reconciliation. For the product-level description of how Braid uses
GitHub as durable working memory, see `docs/10-prd/purpose.md`,
`docs/10-prd/objects.md`, and `docs/10-prd/workflow.md`.

GitHub is canonical for Work Item content and collaboration lifecycle. Braid
uses webhooks for low-latency observation and complete GraphQL/REST rereads for
truth. Arrival order is never canonical order.

## App Identity and Permissions

The configured Braid Agent App supplies:

- the stable Bot/App-authored write and reaction identity;
- repository installation scope;
- webhook deliveries;
- canonical read and `braid gh` write credentials.

An installation is an Issue assignment target only when GitHub has provisioned
it as an Agent App. Ordinary GitHub Apps are not standard user assignees.

MVP App permissions are:

- Issues: write;
- Pull requests: write;
- Contents: write, used by `pr ensure` only for remote refs and an empty
  bootstrap commit when the head has no diff yet;
- Metadata: read;
- Projects: read when Project V2 fields are enabled.

The Agent may separately use its own `gh` and Git credentials. Braid does not
restrict them. A Profile may name one stable GitHub actor node identity; events
whose current action actor exactly matches it are Agent-origin for that Profile.
Writes executed through Braid's durable outbox are correlated Agent-origin as
well. A direct write under any other or shared Human identity is processed as
external activity.

## Webhook Ingress

- Compute HMAC-SHA256 over the exact request bytes and verify
  `X-Hub-Signature-256` in constant time before JSON parsing.
- Persist `X-GitHub-Delivery`, `X-GitHub-Event`, action, repository/object IDs,
  actor, receipt time, and raw payload before acknowledging. Redelivery retains
  the delivery GUID.
- The repository in the payload must be one of the explicitly configured
  repositories for this instance. A signed delivery from another installed
  repository is rejected before scheduling rather than routed through a default
  repository.
- Return 2xx within GitHub's ten-second deadline only after durable ingest.
  GitHub does not automatically retry failed deliveries; reconciliation and
  explicit redelivery remain required.
- Unknown event/action/union variants are durably recorded and trigger
  reconciliation. They are never serialized generically into Agent input.

### GitHub → EventKind Mapping

The GitHub producer maps deliveries onto the platform-neutral `EventKind`
contract; consumers never see GitHub event names or actions:

| GitHub delivery | Internal `EventKind` |
| --- | --- |
| `issues.assigned` / PR assignment to the App (canonical-reread confirmed) | `assign` |
| First Trusted Braid Mention on a dormant Work Item | `mention`, consumed as `assign` (activates the group, exactly like native assignment) |
| `issues.unassigned` (canonical-reread confirmed) | `unassign` |
| Comment/review created, `pull_request.synchronize`, review requested; Trusted Braid Mention on an already-active Work Item | `mention` / `wake` |
| Relevant external body/description/comment edits or deletions, review dismissed, review thread resolved | `invalidate` with durable `requires_attention` |
| `closed` / `reopened` / `merged` | `lifecycle` |
| Correlated Braid App or Profile-actor writes | `origin_echo` (recorded, never wakes or invalidates the same Agent) |
| `ping`, unknown variants | `noop` (recorded as reconciliation evidence) |

A Trusted Braid Mention on a Work Item whose group sleeps because the Work
Item is closed is a `mention`, not an `assign`: closed groups do not wake;
reopen is the designed re-entry.

Subscribe to:

- `issues` and `issue_comment`;
- `issue_dependencies`;
- `pull_request`, `pull_request_review`,
  `pull_request_review_comment`, and `pull_request_review_thread`;
- `project_v2_item` when Projects are enabled;
- repository/installation lifecycle needed to stop routing on uninstall or
  permission loss.

The official [webhook payload](https://docs.github.com/en/webhooks/webhook-events-and-payloads),
[signature](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries),
and [delivery](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks)
contracts are the source authority.

## Issue and PR Activation

GitHub's Agent App assignment is a special product capability, not a permission
granted to every ordinary GitHub App. A live 2026-08-13 probe against the Braid
installation confirmed that adding its Bot login through the standard Issue
assignee API is rejected with HTTP 403. Braid therefore exposes two Issue
activation modes:

1. when the installation is a provisioned Agent App, an `issues.assigned`
   delivery starts a generation only after a canonical reread confirms the
   exact assignee; the activation creates an idle session and does not invent a
   turn;
2. otherwise, the first Trusted Braid Mention on a dormant Issue produces one
   `ActivationIntent` and preserves that same comment as an urgent Wake Event,
   so materialization is followed by the first turn.

Both modes converge on the same activation: native assignment arrives as an
internal `assign` event, while the first trusted mention on a dormant Work
Item is a `mention` event that the consumer activates the dormant group for;
the paths differ only in how the platform expresses the signal, never in
consumer behavior.

Native unassignment is likewise available only in the first mode and must be
confirmed from canonical assignees before entering debounce. The fallback is
not presented as a fabricated assignment.

GitHub does not expose a general native PR Agent assignee workflow. PR
activation therefore has two sources that produce the same durable
`ActivationIntent`:

1. a Trusted Braid Mention in a PR conversation comment;
2. successful convergence of `braid gh pr ensure` for an Issue comment ID.

The configured handle defaults to `@braid`. Permission is checked at delivery
time with current repository permission and requires `MAINTAIN` or `ADMIN`;
`author_association`, sender login, and stale cached permission are insufficient.

GitHub's special Agent App entry points are documented in
[Using agent apps](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/cloud-agent/use-agent-apps).

## Canonical Reads

GraphQL is the primary Context read because it exposes stable node IDs,
minimization, edit timestamps, Project V2, relationships, linked branches, and
review-thread resolution. Braid paginates every connection to completion:

- Issue root, assignees, labels, milestone, Issue Type;
- Project V2 items and every non-empty field-value union;
- parent, subIssues, blockedBy, blocking, duplicate/canonical timeline evidence;
- `linkedBranches` Development refs;
- Issue comments with author, full database ID, created/updated/last-edited,
  minimization/reason, pinned state, and Markdown body;
- PR root, labels/assignees/milestone/projects, base/head ref names;
- native associated Issues through `closingIssuesReferences` and the reciprocal
  Issue connection, with 1:N and N:1 allowed;
- PR conversation comments, reviews, and every review thread/comment page with
  resolution/collapse/outdated/path/line metadata.

Context snapshot connections share one stateless first-page GraphQL operation
(`IssueFirstPages` / `PrFirstPages`). Each connection with `hasNextPage` continues
independently using its existing cursor query; Project V2 is selected only when
enabled. Root and complete native-association markers still bracket every read.
There is no cross-round cache or root-marker shortcut. Associated Issues, project
field values and review-thread comments retain their complete independent reads.
Partial data/errors and unknown unions fail the snapshot, not merely one section.

The local HTTP bill (Projects disabled, single page, empty associations/threads,
no drift) is Issue **13 → 5** and PR **10 → 5** requests. Previously association
connections were read three times (before marker, snapshot, after marker); they
still are, but the snapshot read now shares the first-page operation. Two root
reads and two association-marker reads remain deliberately separate. A comments
continuation adds one request; a full drift retry repeats the bill. An open
associated Issue adds its own Issue bill and each thread/project field connection
adds its pages. These are mock HTTP counts, **not measured GitHub point savings**.

REST supplements GraphQL for endpoints or lifecycle evidence not exposed in a
stable query. Requests pin an explicit supported API version.

On 2026-08-13 the full selection was executed read-only against a live public
`openai/codex` Issue and PR. The Issue query returned labels/comments and
successfully selected Issue Type, Projects, parent/sub/dependency/linked-branch
connections; the PR query returned conversation, review, and review-thread
connections. Every pageInfo flag was checked. This proves field compatibility,
not the content of future repositories.

## Native Association

PR Context includes every direct native association. Braid queries
`PullRequest.closingIssuesReferences` and
`Issue.closedByPullRequestsReferences(includeClosedPrs: true)`; it does not
infer association from title, branch name, ordinary `#123` text, timeline
cross-reference, or commit message.

`pr ensure` establishes a GitHub-native link, preferably through the explicit
Development relationship supported by the target repository and otherwise a
valid closing reference targeting the default branch. Multiplicity is legal.
Removing or changing an edge invalidates affected PR Contexts.

See GitHub's [linking contract](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue),
[Issue schema](https://docs.github.com/en/graphql/reference/issues), and
[Pull request schema](https://docs.github.com/en/graphql/reference/pulls).

## Reconciliation and Tombstones

Schema 11 separates the latest Context/tombstone projection (`canonical_objects`)
from the producer's processed frontier (`processed_objects`). A projection read,
including reading an associated Issue through a PR, cannot advance the latter.
The producer classifies one complete observation outside SQLite, then atomically
compares its expected frontier, admits all resulting events, and commits the new
frontier. Failure or a changed precondition commits neither. Network awaits never
occur inside that transaction. The frontier is an acknowledgement of classified
input, not a second current-content authority or another execution queue.
完整观察在 API 遍历前取得 processed frontier 与已接纳事件 watermark，提交时同时
校验；读取期间新 delete 等事件到达会拒绝整个旧快照。缺失是负向观察，不是更晚的
上游 updatedAt；本地 tombstone 的时间不能压过 API 当前仍可见对象。正向完整观察
可以纠正它，旧 webhook 不可直接复活 tombstone，真正较新的正向 edit 仍受保护。
冲突报告包含 node、类型、双方版本、对象状态与证据来源，不包含正文。

Context 的完整成员集合必须先维护 ledger，再作 Agent 展示过滤。Operational Status
是受管输出；过滤它不表示删除，它的恢复也不产生业务 Wake。展示投影不能充当缺失
推断的输入。这一约束同样适用于 Issue 与 PR conversation。

Event staleness likewise uses admitted change evidence and the processed frontier,
never a Context cache row. Root synchronize/assignment hints do not acknowledge
body changes. Comparable upstream timestamps may reject older facts; lifecycle
suffixes and review-thread projection strings are not lexical clocks. A complete
snapshot older than a classified fact is retried without advancing its frontier.

A correlated title/body version proves only those published fields. A separate
root metadata fingerprint includes relevant non-body facts and native PR edges;
a metadata/edge change can require attention even when `updatedAt` and body did
not change. Missing action provenance is handled conservatively rather than
turning the entire root into an Agent echo.

Change identity also distinguishes a versioned object from an occurrence.
GitHub delivery GUIDs and built-in event sequences identify occurrences;
returning to the same unversioned thread state is not a redelivery. Complete
observation transitions are committed under their expected frontier and may
reuse the latest matching admitted state transition when non-state metadata is
unchanged. `deliveries.effect_event_id` records that reuse explicitly. A missing
new event is therefore not silently treated as successful delivery: its existing
cause must be bound and auditable. PR activation separately requires its exact
request/target event, not an arbitrary dedupe collision.

Visible semantic fingerprints exclude timestamp-only and filtered HTML-comment
changes. Versioned raw-content evidence remains separate for correlation and
ordering. Original comment authorship is usable only for an API-proven unedited
creation; an edited comment's unknown current actor cannot inherit the creator's
trusted-mention authority. Already-active responsibilities still receive ordinary
attention for real revisions when the current action actor is unknown.

迁移建立新的观察边界，不能据此证明历史丢失输入已执行。Schema 12 将已接受且
固定精确 PR、却没有 activation receipt 的错误 applied ensure 重新交给唯一 outbox。
这是原请求的交付恢复，不根据任意 native 关联新建职责；当前 canonical 源/目标、
Profile、生命周期与读取期间事件栅栏都必须重新验证，撤销/阻塞的角色不被自动重建。

Webhook 是观察，不是完整历史。巡检在正常预算下按两档到期：OPEN Work Item，
或仍有 materializing/active/finalizing Assignment 的条目，完整巡检成功后默认
60 秒再次到期；其他已关闭或合并的历史条目为 **30 分钟**。关闭不意味着收尾已
完成，finalizing 不降频。历史条目的 reopen webhook 若丢失，可能需要 30 分钟
加上扫描、网络和配额等待时间才能恢复。Webhook 与前台 Context/turn 语义不变。

`scheduler.reconciliation_seconds` 是 hot 条目的最小成功间隔，不是 worker tick。
worker 每 `min(reconciliation_seconds, 5)` 秒检查本地到期事实与已知预算；本地
检查本身不请求 GitHub。检查额外延迟至多 5 秒，不包括其他对象遍历及网络/预算
等待。正常预算下的周期不是独立于 GitHub 可用性的硬 SLA。

Schema 8 分别持久化完整成功时间和尝试时间。Webhook、关联读取、Context Revision
写入与普通 `observed_at` 不推进成功时间；失败或部分读取也不算成功。尝试之间
至少间隔一个 hot 周期，避免每 5 秒检查变成每 5 秒重试。成功项不会因后续条目
失败或进程重启被重新全读。到期项跨仓库按最久未尝试优先，每次尝试保留巡检回执，
单项失败不阻断后续条目。没有实际遍历的空检查不清除已有降级健康状态。

巡检仍完整读取直接关联和全部 Context connection，不使用跨轮 Context 缓存，
也不根据根对象 `updatedAt` 跳过子连接。只有完整物化后才能推断 tombstone，
不能把预算中止的部分列表当作删除证据。

### Installation GraphQL budget

实例内显式共享一个按 installation 分组的准入 owner。运行期仓库客户端及
`for_repository` 派生客户端共享它，不同 installation 独立。每个安装的锁覆盖
GraphQL 请求至响应记账，确认限流后排队请求不能继续穿透。派生客户端用
`Arc<Octocrab>` 共享自动刷新 token 缓存，而不是复制缓存或每次重新签发 token。
同 installation 的跨仓库派生也无需重复完整 App connect/token/viewer 初始化。

所有 GraphQL 调用（含启动时 repository/viewer 身份读取）统一走 raw transport。
准入失败立即返回，不在请求内 sleep 或循环重试；每个已准入请求及 body decode
合计限时 30 秒。Octocrab transport 隐式重试关闭，REST 429/5xx 仍是可恢复错误，
由持久 outbox 重试，而不是永久拒绝。App discovery/token 和 REST 不受 GraphQL
预算控制。首次确认限流即返回可恢复错误，不能等到后续本地拒绝时才允许写入重试。
明确的权限拒绝仍是权限错误，不作为 installation 耗尽处理。

响应 DEBUG 记录可信静态 operation、foreground/background class、installation、
response resource、请求计数、request ID、HTTP status、latency、used、remaining/reset
配额窗口；repository/work item 仅在日志/span，不是 metric 标签。普通成功不再逐条
INFO 刷预算；准入/限流/失败保留 WARN 事件及响应元数据（若已收到），不包含正文、
token 或 variables。采样证据路由不变。header used delta 仅在可比 resource/reset
窗口内计算，仍可能包含别的消费者，**不是单 operation cost**。

Braid-owned Context/identity queries显式选择 `rateLimit { cost }`，仅服务器实际返回
的值被计入 `braid.github.graphql.points`；缺少该字段是 unknown，不推测为零。
`braid.github.requests` 与 `braid.github.request.duration` 按低基数
operation/class/resource/outcome/cost_known 聚合，独立记录已准入请求与单调时长。
这些指标及上述归因日志的 coverage **仅为 Braid GraphQL transport**，包括 canonical
Context 完整遍历；`resource=graphql` 是固定的 metric 标签。REST（包括 `write_rest`）
和 App/installation-token 请求不在此测量范围，其请求数、时长和成本仍为 unknown，
不能把这些指标视为所有 GitHub 调用的总账。
未收响应/失败也有请求计数；本地准入拒绝没有 HTTP 请求，不计入 requests。
任意未识别 query 名归为 `other`，不改写调用者 query，不把其名字变成无限标签。

主限额耗尽等待 reset，次级限流遵守 `Retry-After` 秒数。缺少提示的限流和网络
故障从 10 秒开始指数退避，上限 300 秒加至多 25% 抖动；后续失败不能缩短已收到
的服务端冷却期限。到期仅允许串行探测，成功响应重置退避。后台在剩余点数不大于
100 时让出余量；缺少 reset 则有界冷却后探测，不永久停摆。这只是前台余量，
不是保证任何大 Context 都能在剩余配额中完成。

预算不足时巡检报告 `degraded/delayed`，保留未完成条目供后续尝试，不强行追赶。
预算/冷却只在进程内共享，不持久化：重启保留对象成功时钟，但首次响应前不知道
剩余额度；独立 CLI 和其他消费者仍可能耗尽共享 installation 配额。自动恢复只
涵盖仍有重试入口的操作，不自动解除业务 `blocked`（操作边界见部署文档）。
本地 HTTP/时钟回归验证机制，**不是线上 installation 消耗测量**。

An object previously observed but now missing becomes a tombstone carrying only
stable ID, last known author/time metadata, and `deleted`. Local state never
retains a deleted body as Context. Older webhook payloads remain delivery
evidence. Ordering is applied only where GitHub exposes comparable versions;
otherwise a lifecycle event causes a canonical reread instead of a guessed
lexicographic comparison.

Cross-surface description detection is Issue-owned rather than inferred from
the generic root `updatedAt`: Braid compares the exact visible Issue description
after HTML-comment removal once, then fans a real change out to every active
direct PR association. Webhook `issues.edited` uses the exact `changes.body`
signal. The new visible description and any derived PR events are committed
together, so repeated delivery, a metadata-only edit, or another Context read
cannot manufacture a second invalidation.

Reconciliation records a previously unseen review thread without creating a
second Wake: the thread's first visible comment/review delivery already owns
that Wake. Only a known thread changing from resolved to unresolved creates the
thread-level Wake; resolved remains a Hard Invalidation.

## `braid gh` and Outbox

`braid gh` follows familiar GitHub target/flag conventions and implements the
MVP write subset under the Braid App identity:

- comment create/edit/delete/minimize/unminimize;
- Issue edit/label/close/reopen;
- PR edit/label/ready/draft/close/reopen;
- review reply/resolve/unresolve;
- idempotent `pr ensure` keyed by the triggering Issue comment ID.

The current Work Item is only a default target. Missing verbs are implementation
backlog, not a permission policy, and ordinary `gh` remains available.

The first shipped write vertical is intentionally small and recoverable:

```shell
braid gh comment create owner/repository#123 \
  --config /absolute/path/to/braid.toml \
  --profile issue-codex --source-turn TURN_FROM_RUNTIME_PROMPT \
  --request-id stable-agent-message-key \
  --body 'Concise public update'

braid gh pr ensure --comment 123456789 --source-turn TURN_FROM_RUNTIME_PROMPT \
  --config /absolute/path/to/braid.toml
```

`comment create` derives the public role from its durable source Group, not the
canonical target or Profile tags. Tags only select role eligibility. The writer prepends one attribution block and removes exact
repetitions of that generated block from the start of the supplied body, so a
caller retry or model mistake cannot duplicate it. `--request-id` is optional;
exact locally accepted request content provides the fallback retry identity, while an
explicit value lets the Agent name retry intent. Command results expose only
bounded operation/target/Profile/lifecycle and semantic GitHub comment/PR
references. Internal write IDs, node IDs, digests, and retry counters remain in
SQLite and never enter Agent tool output. `pr ensure` accepts an optional explicit `--head` and, in a multi-repository
instance, requires `--repository`. It first selects an explicit requested head,
the sole unambiguous Development linked branch, or a deterministic
request-comment-derived ref. It
compares that head to the intended base. When GitHub has no differing commit,
Braid uses the Git Data API to create a commit with the current head tree and
parent, message `chore(braid): initialize implementation request <comment>`, and
stable Braid App authorship, then advances/creates the ref without force. A
concurrent Human advance causes reread rather than overwrite. The empty commit
is the minimum public fact needed because GitHub cannot open a PR without a
head/base difference; it changes no repository file. Braid then creates the
Draft PR, establishes native association, and records ActivationIntent. GitHub
requires Contents write for App installation
[commit](https://docs.github.com/en/rest/git/commits) and
[reference](https://docs.github.com/en/rest/git/refs) creation.

### Durable acceptance and one executor

`comment create` and `pr ensure` perform local configuration, format, Profile and
available source-binding checks before any GitHub await. One transaction records
an immutable request in `write_intents` and its execution obligation in
`github_write_outbox`. The CLI never claims or mutates; it immediately returns
`queued` and a stable public `request-…` ID, including when the service is offline.
`braid gh status <request-ID> --config <path> --json` retrieves the receipt without
resubmitting body content. The public ID is separate from the internal intent ID.
Receipt projections include operation, target, lifecycle, ensure stage/head/base,
and confirmed GitHub references; queued is not published. Same key and digest
returns the same receipt; a changed request conflicts. Raw comment content and a
Profile snapshot are accepted as a typed, not-yet-target-validated request.
Canonical target kind, operation/source association and installation write permissions
are still verified before mutation. This does not expand operation/target rights.

Only the runtime-owned outbox claims and settles writes, with runtime generation,
owner ID and monotonically increasing attempt fences. Network awaits never run
inside SQLite transactions. A known pre-mutation failure stays pending; uncertain
execution requires canonical recovery, never blind replay. Per-task durable
`next_attempt_at` honors the shared installation reset/cooldown and bounded
exponential backoff; the 250ms local worker tick is not a quota probe. Restart
recovers sending work as uncertain, including migrated legacy CLI obligations.
Reactions and operational status continue on the same executor; Built-in uses
its existing upstream adapter and the same fenced claim/settlement API.

Ensure accepts raw comment ID/head/Profile before planning. Remote planning freezes
base/head, request comment and Issue identity, bootstrap authored time and PR
Profile once in `implementation_requests`. That table is domain progress, not
another queue. Recovery reuses `planned → head_ready → pull_request_ready →
associated → activation_pending`, including a confirmed PR number even when it
is no longer in open-PR listings. Native association and the delivery-key-idempotent
initial Activation must finish before applied. Changed activation Profile
configuration is rejected rather than silently activating another Profile.

`PublicationSource` is the typed extension seam for source-aware maintenance:
agent/assignment/generation, Profile/revision, session/turn and Context revision
are captured from durable worktree/session state and rechecked before mutation.
Completed turns retain accepted obligations; pending invalidations, replaced
sessions and newer assignment generations fence old output. An uncertain comment
is first looked up and acknowledged if uniquely found, even if the source was
subsequently fenced; it is never resent under a stale fence. Current launchers
only supply `BRAID_AGENT`, not a runtime-signed invocation handle. Worktree lookup
is therefore a provenance check, **not** an environment-based authority proof or
an OS sandbox. The source-aware memory component must provide an explicit bound
invocation/operator distinction; removing an environment variable cannot be
claimed to be securely prevented by this layer.

Every Braid-owned mutation first commits an immutable write intent. States are
`pending`, `sending`, `uncertain`, `applied`, `conflict`, `ambiguous`,
`rejected`, and `superseded`. A timeout becomes `uncertain`; Braid rereads
canonical state before retrying. Known-object updates/reactions converge by
desired state. GitHub creates have no persisted queryable exactly-once key, so
an uncertain comment create accepts only one matching App author, target, exact
body, and creation-window candidate; multiple candidates become `ambiguous`.
`pr ensure` additionally reconciles the request comment ID, deterministic head
ref, bootstrap commit ancestry, open PR head/base, and native associations.

## Reactions and Status Comments

After durable ingest Braid adds `eyes` to each new external comment. For the
exact Trusted Braid Mention only, it maintains the reaction lifecycle defined
in [`lifecycle.md`](lifecycle.md). GitHub reaction create is treated as
idempotent; removal lists and deletes only Braid-owned reaction IDs.

Operational Status Comments are App-authored projections keyed by
`(profile, surface, assignment generation)`. Braid edits one stable comment per
key, excludes its ID from Context, and never hides debug metadata in the body.
Profiles choose Issue, PR, or both direct surfaces. Status covers context
pressure/unavailability, provider/transport unknown, blocked Profile/worktree,
and ambiguous/rejected Braid writes.

公开故障投影不是只追加的告警。唯一 outbox owner 领取写入前，按持久账本对账已有
status：最新 active generation 的当前 Session 中，Provider 已接受的 running turn
或正常 completed turn 才是接手证据；仅创建 assignment、替换记忆或工作树干净不算。
同代在故障之后有新的确认，可将原 comment 标为恢复；新代确认后，旧代 comment
标为有时间的历史故障并说明由哪代、哪个 Profile 接替，旧代并未被解阻。正文明确
这是最后确认的执行状态，不是代码、review 或产品完成，也不是持续可用保证。
正常 turn 不因此新增评论；重启从账本补偿，没有额外队列或周期性模型调用。

status 的 desired publication 与远端 comment 身份分开。每个 outbox publication
持久绑定原 status key；旧 create 在发送中或 uncertain 时，新投影等待它对账，旧
回执仍能补回 remote ID，但不能覆盖较新投影的生命周期。确认 ID 后新投影改为
更新同一 comment；未发送旧投影可 supersede。重复故障不更换回执指针，故障恢复后
再次发生则是新的 publication，不由正文永久去重。已被新代接替的旧代故障晚到，
不会把公开状态降回当前失败。若旧 create 的恢复匹配到多个 App 评论，则远端身份不唯一：
同一工作项、Profile 和 generation 的后续 create 也记为 ambiguous，不再 POST，更不猜测
PATCH 目标；不影响其他键。只有已确认的远端身份才允许更新原评论。Schema 13 对已有 status/outbox 回填映射，不改职责状态。

For initial `pr ensure`, the source is the calling Issue assignment; the frozen
PR Profile selects the new implementation role and must not replace that source.
Cwd/session resolution checks the durable source role and treats an explicit
comment Profile as an assertion, not an identity override. REST publication shares
installation cooldown/admission with GraphQL while keeping REST response counters
out of GraphQL point accounting; server Retry-After/reset deadlines reach the
outbox's durable next-attempt deadline.

### Source-bound title/body maintenance

`PublicationSource` remains the single durable provenance model in
`write_intents.source_binding`. Agent CLI entry requires a runtime-prompt
`--source-turn`; resolution cross-checks the actual canonical cwd with the active
worktree, assignment generation/repository/role, configured Profile materials
revision, current session and Context revision, observed running or completed
turn, invalidation and replacement fences. An unobserved starting turn never
borrows an older completed identity. The selector is not a bearer capability.
`--operator` is explicit, forbidden in Agent runtime worktrees, and recorded as
Operator. This does not claim hostile same-UID process isolation.

`pr read`/`issue read --output PATH` exports a typed canonical title/body snapshot
including updated version and SHA-256 digest. `pr edit`/`issue edit --snapshot PATH`
accepts `--title` and/or `--body-file`, with Profile assertion, invocation binding
and stable `--request-id`. It serializes `AcceptedRequest::Edit` into the existing
intent + unique `agent_write` outbox transaction before any GitHub access. No
migration or additional executor is required. Only the existing runtime owner
may execute; all existing attempt/restart/installation cooldown fences apply.

The own-Item permission and native direct cross-surface association are checked
separately from source attribution. PR source may edit only its own PR or its
current directly associated OPEN Issue; Issue source may edit only its own Issue.
Related comments remain valid in both directions. The source role never changes
to the target's role. PR ensure additionally requires its Issue source to own the
request comment's Issue.

Before body mutation the executor reads a canonical snapshot, rejects a changed
digest as `conflict`, revalidates target state/association/installation rights,
reads again, and checks the source/owner fence immediately before PATCH. It
preserves recognized existing ensure-request metadata and explicit closing-source
blocks (including ATX/Setext headings) when rendering source attribution once.
Complete supported top-level paragraphs/headings are prefixed before replacement
Markdown, outside new unclosed fences/comments. Context-dependent existing sources
that cannot be reliably preserved are refused for body replacement; title-only
edits retain them unchanged. Neither ordinary closing words nor new code, lists
or links are forbidden. This is bounded preservation of known sources, not an
emulation of GitHub's private parser or a guarantee that relationship sets cannot
change.

An authorized body may add native closing text. `braid gh` is an identity surface,
not a Markdown semantic sandbox, and adds no dedicated relationship operation.
New body text never supplies authorization to edit a different Issue: execution
queries current GitHub canonical direct edges. A GitHub-confirmed new edge can
expand the PR Agent's maintainable open Issue set under existing source/Context
fences. Plain references and closing words alone grant no authority or Group
activation/scheduling. Canonical graph reconciliation remains independent of
Agent-origin wake suppression; explicit handoff constraints remain separate.
No write-then-rollback is used to simulate CAS. An applied edit confirms content,
not relationship-set equality.

PATCH's returned title/body must match; an unconfirmed send
stays uncertain and is read back before retry. Exact resulting content confirms
applied even if authority has since expired; any different version is a conflict,
not proof of success. This is optimistic conflict checking, not atomic GitHub CAS;
the final read/PATCH interval remains a documented concurrency limitation.

### Explicit existing-generation PR handoff

`writer::handoff` reuses source-bound `Invocation`, `AcceptedRequest`, and the
single `github_write_outbox`. Schema 10 `pr_handoffs` binds each accepted intent
to the current active target assignment and records its unique dedicated Wake
reference. Operator/PR sources, inactive/blocked/missing targets, finalizing
assignments and non-OPEN sources/targets are refused. Queued/uncertain Issue
body edits must be resolved before acceptance/delivery; no workflow DAG is added.
The existing GitHub reads recheck OPEN state and native `closingIssuesReferences`
within the connected repository installation, never comments or request Markdown.
All network awaits are outside SQLite transactions.

In the outbox attempt settlement transaction, Braid rechecks the source fence,
current target assignment and scheduling eligibility; inserts the ordinary
`wake/pr_handoff` event once; merges its batch with normal quiet/count; and commits
the receipt/outbox state. A stale fence rejects without partially delivering a
Wake. At dispatch, an ordinary claimed handoff batch freshly materializes and
renders the full canonical PR/associated Issue Context before provider send.
Failure defers the unsent claim. This appends current working data to the existing
session, not a Context reset, urgent steer or parallel turn; it does not advance
the session's replacement fence. The boundary rechecks the bound target and
requires each handoff source still appear as an OPEN directly associated Issue.
No transport, pagination, canonical query or OriginEcho rule changes are involved.
