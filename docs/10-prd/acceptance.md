# End-to-End Acceptance

本文件持有真实产品验收判据。GitHub 后端的下述 campaign 要求真实工作项、
发行 binary、公网 webhook、原生执行器及可观察 Agent 行为；内建平台的入口与
判别事实见末尾内建空间章节。编译和机械对象操作各自只支持它们实际覆盖的结论。

## Boundary and Fixture

The campaign uses:

- a tagged, checksummed macOS arm64 Braid artifact installed into a clean
  directory without source, Python, or Cargo;
- the real `xiaoland/braid` repository or an approved disposable mirror at a
  pinned revision;
- a dedicated non-production Braid Agent App and webhook secret;
- a free Cloudflare Quick Tunnel to loopback ingress;
- the pinned real Codex app-server and a real Coding Agent login;
- one dedicated repository checkout for the Issue Profile and Braid-provisioned
  PR worktree;
- one maintainer actor and one actor without MAINTAIN/ADMIN permission;
- an OTLP endpoint suitable for retaining the campaign's full sensitive trace
  payload.

Drive the product only through GitHub, public `braid` CLI/health/status, process
control, provider executable, Git, installed artifact, and OTLP export. SQLite
inspection, internal Rust APIs, fake providers/GitHub, and hand-injected inbox
events are never pass oracles.

验收按本页描述操作真实产品并保留原始证据。自动测试、模拟和包 smoke 可验证
局部实现，但不能替代本页的真实产品验收。GitHub 对象和人工判断仍是产品验收依据。

## 开发回归与产品验收的分层

修改代码时，先运行所属边界的回归，再由 CI 验证 macOS arm64 和 Linux x86_64。
每个测试须能指出具体可信回归；不同层保护不同风险时分别保留。不要为了减少
测试数量删除协议、数据库兼容、授权或停止证明，也不要把测试自建的配置、状态机
或回执当作生产实现的证明。

| 契约 | 主要自动证明归属 | 仍需真实验收的部分 |
| --- | --- | --- |
| 配置、凭据导入、headless setup、公开回执 | `tests/` 中实际 binary 子进程；配置与 doctor 的局部不变式 | GitHub App 转换、登录、provider 认证与安装后的工具环境 |
| GitHub 准入、完整读取、发布及 uncertain 写入 | `producer`、`context`、`github`、`writer` 的生产 owner，外部 HTTP 固定夹具 | 实际 webhook、GitHub 权限、native 关联及跨仓库行为 |
| durable intent、租约、迁移和重放 | `store` 的真实 SQLite actor；旧 schema 建库、备份还原、关闭重开 | 已发布旧 binary 升级及现场恢复，不能以直接造库代替 |
| Context 替换和 provider 事件 | `group`、`provider` 的真实编排；受控外部会话通知和停止回执 | 真实模型消费新 Context、适配器后台执行停止、长轮次中撤销职责 |
| 内建对象与观测存储 | `platform`、`console` 的 HTTP→SQLite/Git 回归、真实 Console 二进制和两页并发编辑；独立 OTLP 编码器输入 | 完整操作旅程、实际 Harness 与跨宿主运行 |
| 原生执行与材料 | Node 真实进程树、已安装的 Pi background 插件与 RPC；Profile snapshot 和 auth 文件重新读取 | 其余插件组合、真实模型工具调用及跨宿主进程行为 |
| 采样与完整敏感证据 | `telemetry` 中真实 SDK→HTTP→protobuf；每次独立子进程初始化 SDK | 生产 Collector 的权限、保留、查询及完整长旅程关联 |

默认入口与 `.github/workflows/ci.yml` 一致：

```shell
cargo fmt --check
agent-build cargo check --locked --all-targets
agent-build cargo clippy --locked --all-targets -- -D warnings
agent-build cargo test --locked --all-targets
node --test native/pi/tests/*.test.mjs
```

`agent-build` 是本机编译串行入口；CI 直接运行相同 Cargo 命令。局部迭代可加
`--bin braid <模块名>::` 或 `--test <文件名>` 筛选。Linux CI 另运行
`cargo test --locked --bin braid --features opentelemetry-otlp/internal-logs telemetry::tests`，
实际启用 SDK 内部错误诊断，验证 Collector 返回的敏感正文不会进入普通日志。
这不改变发行 binary 的 feature。标为 ignored 的 `telemetry_child` 只供父测试
用独立子进程调用，父测试负责检查退出状态与所有输出；不能单独计作通过。

Linux CI 在同一次 Cargo 构建后启动一个独立平台，从两个 Chromium 页面编辑同一
Issue。先提交的一页保存成功，过期页面显示该对象的修订冲突并保留正文；取消刷新
保留草稿，确认放弃后重新打开才读到已提交正文。复跑时使用本次构建的二进制：

```shell
npm ci --prefix tests/browser --ignore-scripts
BRAID_BINARY=/absolute/path/to/braid CHROME_BIN=/absolute/path/to/chromium \
  node --test tests/browser/draft.test.mjs
```

Pi 安装变更触发独立的 Linux `pi-runtime` 工作流：按 lock 安装 background
插件，启动安装包中的 Node/Pi，以真实 RPC 读取状态、运行工具、关闭执行并离线
检查回执。安装步骤需要访问发行站点和 npm；模型未参与。其它插件组合和含模型
会话各自单独记录。发行工作流在每个平台上传前运行
`scripts/verify-package.sh DIST RELEASE_TAG RUST_TARGET`；它从校验和指定的
归档中运行 binary，核对版本、示例配置及迁移后重开的数据库。

负向用例应断言错误阶段、类型或关键诊断，并有能够到达该守卫的合法前置条件。
恢复用例应先让真实 owner 写入，关闭并重新打开，再从消费者取回结果。新缺陷
修复保留同一输入下修复前红、修复后绿的证据；缺少这种证据时说明原因。删除或
迁移测试前，核对原断言在保留测试中的归属；仅重复编译同一模块不算额外证明。

全绿支持上述局部合同，不授予产品发布资格。真实 campaign 仍按下列场景记录
`pass`、`fail` 或 `unavailable`，尤其不得把 Provider terminal、HTTP 请求受理或
命令 `prepared` 回执当作任务完成、持久采用或停止确认。

## Campaign Setup

Create a real Issue whose description contains a distinguishable design fact,
one well-formed HTML comment containing a forbidden sentinel, and an ordinary
visible sentinel. Add labels and at least one available relationship/Project or
Development branch. Configure one `issue` Profile and one `pr` Profile with
different instructions that produce externally distinguishable behavior.

Set the campaign Quiet Window to 30 seconds, Wake count threshold to eight,
reconciliation interval to 60 seconds, and OpenTelemetry sample ratio to 1.0.
Record the installed Braid/Codex/Git/gh/Wrangler versions, config/profile
revisions, repository revision, App identity/permissions, process start time,
and pristine worktree states before activating Braid. Record whether the
installation exposes native Agent App assignment or uses the Trusted Braid
Mention fallback; the campaign must not claim one mode while exercising the
other.

### 固定真实 Context 夹具

在专用 campaign 配置上记录 Issue、PR、直接关联的已关闭 Issue、可见评论、
折叠/删除评论 ID、一个仅出现在第二页的评论标记，以及各自的预期正文片段。
独立预期来自操作者预先创建的工作项和 GitHub 页面。固定夹具后，在权限受限的
证据目录执行以下公开命令。命令会在 campaign 的本地账本中记录读取到的修订。

```shell
"$BRAID_BIN" context issue "$ISSUE" --config "$CONFIG" --page-size 1 > "$EVIDENCE/issue.md"
"$BRAID_BIN" context issue "$ISSUE" --config "$CONFIG" > "$EVIDENCE/issue-default.md"
"$BRAID_BIN" context pr "$PR" --config "$CONFIG" --page-size 1 > "$EVIDENCE/pr.md"
"$BRAID_BIN" context pr "$PR" --config "$CONFIG" > "$EVIDENCE/pr-default.md"
cmp "$EVIDENCE/issue.md" "$EVIDENCE/issue-default.md"
cmp "$EVIDENCE/pr.md" "$EVIDENCE/pr-default.md"
```

比对两种分页得到的字节后，再逐项核对独立预期：完整可见正文和后页标记必须
出现，折叠/删除评论只含各自的元数据，隐藏标记不出现；PR 的关联 Issue 身份与
数量符合 GitHub，已关闭 Issue 的旧正文不出现。记录每项输入、对象身份和
pass/fail/unavailable。对超过 hard limit 的独立配置，分别保留 stdout、stderr
和退出码：必须因 Context 超限失败且 stdout 没有部分正文。缺少对应夹具时
该项为 unavailable。关于 Agent 实际收到、采用哪版 Context，按旅程 1、3、4 取证。

## Required Journeys

### 1. Activation and Initial Context

If the installation exposes native Agent App assignment, assign it to the
Issue. Braid must create one Issue Agent generation and physical Codex thread,
inject complete current Issue Context, and remain idle without inventing a turn
or Agent comment. If it is an ordinary GitHub App, create one maintainer
Trusted Braid Mention on the dormant Issue instead. That same comment must
produce one generation and one initial turn after complete Context injection;
Braid must not fabricate an assignee or a second Wake. In both modes the
Context must:

- use plain `GitHub Issue: owner/repo#number` identity;
- include title, description, selected metadata/relationships, and visible
  comments in deterministic order;
- include the visible sentinel and exclude the HTML-comment sentinel;
- contain no JSON envelope, GraphQL node IDs, duplicate URL, Operational Status
  body, or folded body.

In native assignment mode, unassign and reassign once. Unassignment must wait
for debounce and stop the old generation; reassignment must create a new
generation/session, not resume stale provider memory. The ordinary-App fallback
does not claim this native lifecycle.

### 2. Debounce, Count, and Trusted Mention

Create one ordinary Human comment. It must receive `eyes` after durable ingest,
must not show `rocket/+1/confused`, and must not start before the 30-second quiet
deadline. The resulting Agent message is published by the Agent itself under
the configured identity and attribution.

Create eight distinct ordinary comments faster than the Quiet Window. They must
coalesce into one turn released by the count threshold. Then create:

- an unprivileged `@braid` comment;
- `@braid` inside code, quote, and HTML comment;
- one maintainer visible `@braid` comment.

Only the last bypasses debounce. It receives `rocket` after provider acceptance
and `+1` after normal terminal. Edit that mention after delivery; while the turn
is active the new version must steer the same expected turn, not create a
parallel turn. A controlled unexpected terminal replaces `rocket` with
`confused`. Ordinary turn failure instead updates Operational Status and never
adds a terminal reaction to its trigger comments.

### 3. Canonical Lifecycle and Context Replacement

With an Issue turn active, edit a visible comment and the Issue description,
then minimize one visible comment and delete another. Braid must fence the old
Context Revision, safely interrupt/settle the old turn, create a new physical
Codex thread, and inject current complete Context. The replacement must contain
new description/comment bodies, only metadata for the minimized/deleted
comments, and no stale body or provider transcript.

Repeat equivalent lifecycle on an idle active group without posting another
comment or mention. Context/session replacement must preserve one ordinary
pending input, and after debounce the Agent must handle the revised agreement.
Its observable behavior must use the new description, not the superseded
requirement; it may discuss, revise, clarify or explicitly wait. Repeat delivery
and restart while waiting: neither may silently settle the input or add a second
turn for the same revision. Unminimizing the comment restores its body and
creates a Wake Event. Older-after-newer webhook redelivery must not regress the
projection or create a duplicate turn.

### 4. Implementation Request and PR Profile

Have the Issue Agent publish a concise implementation request and call
`braid gh pr ensure` with that Issue comment ID twice concurrently. Exactly one
Draft PR, native association, ActivationIntent, PR Agent generation, PR Profile,
and dedicated worktree must result. When the selected branch initially equals
base, the PR contains exactly one Braid App bootstrap commit whose tree equals
its parent and whose message identifies the request comment; it changes no
file. A second distinct request comment must independently deliver activation to its
own PR and start that authorized PR Agent; 1:N and N:1 associations must not be
rejected. An applied ensure receipt must report durable activation delivery,
separately from provider readiness/completion. PR existence alone is not pass.

The PR Agent's initial Context must contain all current directly Associated
Issues followed by the PR section. Open Issues are complete; a closed Issue is
metadata/reference only. It must omit head SHA, commit/diff/check summaries and
normally reviewers. The Implementation Agent must operate from the dedicated
worktree, create a real diff, verify it, publish concise PR comments itself, and
use ordinary Git/gh freely.

Edit an open Associated Issue description while the PR Agent turn is active.
After debounce it must Cross-surface invalidate and replace the PR session with
the new Issue design. Change a label/comment instead: it may schedule a PR turn
and must refresh Context before that turn, but must not interrupt the active PR
turn. Close the Issue: its description/comments disappear from future PR
Context while reference/state remain. Reopen restores full Context.

在受控旧版 fixture 上重现“PR 已创建、激活未送达”的历史缺口，再升级：使用旧版
对同仓库第二个不同 Implementation Request 创建 PR，以公开 receipt/status 记录缺失，
不用 SQL 制造状态。升级前先修订 Issue、更换原 Source Context；新版仍应安全补交
固定 PR 的原实现职责，不重推分支、不建重复 PR，首个 PR turn 必须读取当前设计与
已接收的 review。另测源或目标关闭、关联移除、目标已有 blocked/retired 历史，均
不能因恢复而自动解阻或新增 generation。后续评论复用同一受管 head 时，回执应沿用
原 implementation request；不同 head 的独立请求仍支持多个 PR。

### 5. Origin and Working-Memory Maintenance

Use `braid gh` from each Agent to update its description/body and create one
attributed concise comment. The correlated webhook echo enters future canonical
Context but does not wake/reset the originating group. Confirm the quote-block
Profile/role attribution cannot be supplied or altered by the Agent body.

Then let the Agent perform an ordinary direct `gh` edit under its Profile's
configured stable GitHub actor. Braid must not block it, and the exact actor
match suppresses self-wake/reset. Repeat under an unconfigured/shared Human
identity: it remains allowed but is processed as a normal external change and
may wake/invalidate. This is the explicit autonomy/attribution tradeoff.

Have the Human revise an Agent-maintained description while preserving its old
Braid attribution block. It must still be handled as a Human revision. Repeat
with webhook delivery unavailable so canonical reconciliation observes it;
original authorship and displayed attribution must not suppress the newer input.

### 6. Close, Merge, Reopen, and Finalization

Close an active Issue and an active PR without merging. Current turns must not
be interrupted; each group receives at most one Finalization Turn, then sleeps.
Additional deliveries while closed grant none. Reopen and observe one current
Context plus one ordinary debounced turn.

Merge the PR in the controlled fixture. It receives at most one Finalization
Turn and then retires; later events do not reactivate it automatically.

### 7. Context Pressure

Run with a campaign-only low Context byte budget. At soft pressure Braid still
starts the complete turn and updates status. Above hard pressure, and separately
with an intentionally unavailable pagination/permission boundary, it must start
no provider turn and update one mutable `context-too-large` or
`context-unavailable` Operational Status Comment on configured surfaces.
No partial, truncated, or generated summary may reach Codex.

### 8. Synchronization, Restart, and Unknown State

Exercise duplicate delivery, temporary tunnel loss, 60-second reconciliation,
and an out-of-order edit. GitHub state must converge without duplicate logical
turns, reactions, status comments, or `pr ensure` PRs.

While the Issue Agent is idle, lose a body-edit webhook and let another complete
Context read (including an associated PR read) see the new description first.
Periodic reconciliation must still deliver the changed agreement to the Issue
Agent without an extra mention. Repeat with only a timestamp or hidden HTML
comment changed: no new work is required. Recover the Tunnel without restarting
Agent execution: the public signed ingress and the App's registered URL must
agree before reporting connected; failures must be visibly degraded.

Restart Braid while debouncing, while a reaction/status write is uncertain, and
while a provider turn is active. Repository owner lease, batches, outbox, and
same compatible physical thread must converge without parallel turns. Disconnect
app-server without terminal evidence: Braid keeps the turn unknown, preserves
the trusted mention's `rocket`, and updates Operational Status rather than
claiming success/failure.

在可复用旧版 fixture 中发布一条真实 Operational Status，观察其 webhook 被接收后
读取完整 Context。该 comment 可以不进入 Agent Context，但不能因此被认成 GitHub
已删除。升级后，旧错误 tombstone 对应的评论仍在 canonical API 中可见时，对账应
自行恢复且 status 本身不产生业务 turn；不允许通过直接改库作为 pass oracle。对真正
较新 edit、读取期间到达的 delete，保留乱序保护并以安全元数据说明冲突来源。

公开状态恢复验收：保留一个已发布 Provider unavailable 的旧 generation，再通过
受支持入口建立新 generation。仅 assignment/materialization 完成不能显示恢复；
Provider 确认接手或正常终止后，原 status comment 应更新为带最后确认时间、明确
新 generation 接替的历史故障。旧 generation 仍 blocked，新状态不暗示实现完成。
在已有恢复结果但未更新评论的现场升级，也应只靠账本补偿，不需要再调用模型。
重启、同一故障重复发布及旧代故障晚到，均不新增重复 comment 或恢复旧职责。
旧 create 已远端成功而本地回执未落库时，必须先对账并沿原 comment 更新，不能以
再次 POST 掩盖恢复；blocked、unknown 或未被 Provider 接受的新代不能通过此验收。

### 9. Telemetry, Migration, and Distribution

With sample ratio 1.0, the retained OTLP trace must correlate webhook receipt,
canonical reread, Context materialization, scheduler/session/provider lifecycle,
GitHub writes, and terminal result. It must contain the controlled comment body,
summary, raw webhook payload, provider transcript/input/output, credential
sentinel, and local path sentinel. Large payloads must be log/event bodies rather
than metric labels. Metrics remain available independently of trace sampling.

Repeat a bounded run at ratio 0.10 and verify trace-level consistent selection:
a retained trace has its complete evidence and a dropped trace does not export
orphan spans. Enable incident mode at 1.0 and verify all new root traces export.

Install a prior schema artifact, create its DB, upgrade with the candidate, and
restart. Verify the forward-only migration ledger/checksums and retained
binding state. A binary declaring the DB schema too new must refuse startup.
Perform one declared compatible binary rollback; for an incompatible fixture,
restore the pre-migration backup rather than executing a down migration.

## Timing and Result Model

- ordinary one-event turn: no earlier than 30 seconds after the latest Wake and
  observable by 45 seconds absent recorded GitHub/provider outage;
- eight-event threshold or trusted mention: provider acceptance/reaction by 15
  seconds;
- reconciliation-only missed event on OPEN/live-assignment Work Items: observable
  processing by 110 seconds under normal budget with a 60-second successful
  traversal interval (includes up to five seconds of local due-check delay);
- closed/merged history without materializing/active/finalizing Assignments:
  missed-event recovery is due 30 minutes after the last complete traversal,
  plus up to five seconds of local due-check delay and processing time; quota or
  network pressure explicitly reports degraded/delayed instead of meeting these
  normal-budget timing targets;
- unassignment and Cross-surface description invalidation: debounce before
  retirement/interruption.

Each assertion is `pass`, `fail`, or `unavailable`. `Unavailable` never counts
as pass. Public evidence includes Work Item/comment/reaction IDs and timestamps,
App/actor identity and permissions, native associations, Draft/Ready/lifecycle,
refs/diff/checks, packaged binary/version/checksum, process control, and Human
verdicts. OTLP and provider evidence may prove Context/telemetry-specific
assertions but cannot substitute for public product behavior.

The golden Issue-to-PR journey must pass without corrective operator action. A
fresh installation and fixture must pass the entire campaign three consecutive
times before the implementation PR becomes ready.

## 多 repository instance 的验收边界

多仓库验收使用同一个 GitHub App、一个 webhook/tunnel 入口和两个已配置
repository。两个 repository 使用不同的 Profile/user_instructions 与独立
source checkout，最好包含相同 Issue 编号以证明裸 number 不成为身份。分别
触发 trusted mention，核对：事件只进入所属 repository 的队列；Context 标题、
正文和评论不混入另一仓库；worktree 路径按 repository 隔离；comment/reaction、
status 和 `pr ensure` 都使用目标 repository 的 scoped client。第三个已安装但
未配置 repository 的 signed delivery 必须在调度前拒绝。跨 repository native
association 仍按既有 PR Context 合同由目标 PR 的 scoped client 读取；它不会把
另一个 repository 纳入调度、Profile 绑定或共享工作图。当前 MVP 桥接仍要求
同一 instance 的 Profiles 使用相同 adapter_type，不宣称混合 Codex/Pi 多仓库。

## 内建空间的验收边界

内建平台的操作验收从公开 `platform/init/object/serve` 入口进行：没有 GitHub App 配置时，
创建、指派、讨论、关联 PR、发布与采用实际候选，重启后仍能读写并投递剩余输入。
核对对象、队列、Git 引用和具体执行状态。机械对象操作不代替模型实际消费和
多 Agent 协作效果；需要判断完整 Agent 工作经历时，应在获授权的真实模型任务上
沿同一流程取证，不构造 provider 或修改私有执行状态。当前隔离操作结果归任务
记录，不据此宣称完成下述 GitHub campaign 或完整模型验收。

## Braid Console 的实际验收

Braid Console 不依赖 Factory。内建平台场景使用独立真实 Git checkout，先只配置
平台而不启动 Harness，实际创建并讨论 Issue，再接入仅传输 Harness，核对对象 revision、事件和
命令回执；两个页面并发编辑时旧 revision 必须明确冲突且保留草稿。同一 request
ID 取回同一结果，不重复创建；不同命令不能复用该 ID。

通过页面从真实候选分支建立关联 PR，阅读实际 diff，明确发布配置 source 或
登记 worktree 的已提交候选，再合并至未被 checkout 的 base。核对发布 OID、
持久合并意图、base 新提交及具体失败；对象关闭和原生执行停止分别取证。

由真实 OpenTelemetry SDK 向 Console 发送三信号，核对原始封包、解码属性、
采样边界、接收与发生时间，以及原件下载。重发保留传输事实；坏 protobuf、gzip
和超限请求保留具体状态与原因，Collector 重启后仍可按游标阅读旧记录。真实
存储不可用返回可重试 503，不把正常遥测批次误判为坏数据。

通过页面启动和停止 runtime，接受回执不能代替就绪或停止确认。runtime 已停
时 Collector 与对象接口继续可用；未确认停止不可启动替代执行。完整 Agent
验收还需获授权的真实 provider/session/turn 运行，编译及无模型操作不替代这一
证明。本轮不推导新的模型、GitHub 或发布授权。

平台与 Harness 边界必须通过实际操作取得反馈：平台无 Harness 时可持续读写；
停止并更换 Harness 后同一对象、历史和事件仍存在，新消费者拥有独立 cursor；
撤销 writer grant 后，相同 actor 和对象 revision 的迟到命令拒绝。旧 Local 数据
显式导入保留身份及回执，原数据库保留且旧写入口不能继续使用。Console 对任一
上游提供同一 Harness 控制合同；没有真实 GitHub 授权时只核对配置/入口，不宣称
完成 GitHub 网络或 Agent 全链路验收。

## Root 自主运行验收

有限运行需要独立的真实模型资格。仅传入 description，由 root 自行形成可分工子项、
实际指派、创建并实现 PR，消费依赖成果并形成明确交付 commit；开发 Agent 不代写业务答案。
在同一记录中分别确认创建、启动、成果采用和终态，不以手动对象操作或编译代替自主行为。

另外实际核对 root 提前关闭、静默未完成、合并冲突、停止和同 run 接续：提前关闭须保留
未完义务且撤销旧 writer，静默不报完成，未确认停止不得替代，同一输入不重复建 root。
普通 serve 与其对象无关的运行保持原合同；并发参数只按在途 Braid turns 判断，不冒称
验证了原生 subagents/background 或宿主总资源上限。候选应用的实际业务验收单独保留。
