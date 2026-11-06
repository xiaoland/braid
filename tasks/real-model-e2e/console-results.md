# 真实模型端到端 Console 回读

Console 浏览器由 console_web 独立只读操作，运行控制与对象写入由旅程 Owner 负责。此记录不替代生成应用验收，不把 OTLP 接收、Provider 退出或请求受理当作业务完成。屏幕与 DOM 原件保存在 `/Volumes/WorkSSD/Development/braid-e2e/builtin/browser/`。

2026-10-11 16:03（Asia/Shanghai）初始内建回读：Console `http://127.0.0.1:14318/`，平台 `http://127.0.0.1:18282/`。权威 supervisor 为 running，Generation `01a129fb-b381-7433-ad29-077d7539032a`，Provider 启用且 health.provider=connected、ingress=builtin；ready=false，已向 runtime Owner 回报其正在修复的 ingress 判断问题，尚未宣称就绪。Collector 已接收 27 条、7 批次，资源实例 `real-e2e-builtin-minimax`、服务 `braid-real-e2e-builtin`；此计数仅为打开概览时的快照。

平台 Issue #1「20261011-minimax-peer-upstreams 离线笔记 CLI」open、revision=3、作者 operator、指派 issue-minimax。Harness 独立账本展示 active Assignment、Pi session 及 Worktree，upstream cursor=3；当时 Turn 列表尚为空，随后定向 Session 日志查询已显示 Turn `01a129fc-ff4c-7cc1-84a2-049adc4b9ae1`、Trace `189f0d83a5fc34ba047dcfe426b75f2d`。原生会话 ID 是 WorkSSD 的 Pi jsonl 路径，和 Harness 登记一致。初始化 observation 不带 Turn 时界面明确显示「Turn 未关联」，未编造完整会话。只读取当前 Session 的首个有界页面，没有展开完整 rollout。

证据：`initial-overview.jpg/.txt`（完整 supervisor 与 Harness snapshot）、`initial-issue.jpg/.txt`、`initial-session-filter.jpg/.txt`。当前仍等待 Owner 的健康修复、PR 候选与最终停止节点，未完成本轮端到端资格。

16:06 修复后回读：新 Generation `01a129ff-19d0-73a3-93e2-b6ae52a9e24c` 为 running、Ready 是、Provider 启用；Collector 1078 条/120 批次，仍只有上述真实实例，没有伪造第二来源。PR #2 作者 issue-minimax、open revision=3、指派 pr-minimax，正文关联 Issue #1，Head `e2ae347be95a9d49f015b5c33cb36bf6d4dee553`、候选未发布。此阶段不判定业务实现完成；空提交的独立 Git 验证由根负责。

独立账本显示 Issue Session resume_count=1，PR 原生 session `2026-10-11T08-05-48-190Z_01a129ff-201e-7542-8b4b-2b40dd231ee0.jsonl`、PR Turn `01a129ff-20c6-7a73-a19b-13a3a1dd5ac4`、upstream cursor=9。浏览器按 instance + 完整 Provider Session + Turn 三项精确筛选，原生事件记录 #842 显示 Trace `4a904799b3532af8b73ec56b82a07afa`。只展开该单条 get_state 请求属性：provider.kind=pi、work_item.kind=pr/number=2、Assignment `01a129ff-1ae1-7b72-a0e3-cb0924fcc230`、generation 和 Turn 均与账本匹配，observation.correlated=true/sample=true。provider.turn.id 为空时保留原值，不当成已登记原生 turn ID。

证据新增 `ready-overview.jpg/.txt`、`bootstrap-pr.jpg/.txt`、`pr-turn-session.jpg/.txt`、`pr-turn-attributes.txt`。尚等待真实业务候选发布与最终停止/Collector 留存节点。

16:10 等待候选期间的一次独立指标回读：指标页已收到 22 条原始记录，真实 `braid.provider.lifecycle.events` 显示 Pi prompt accepted，`braid.provider.payloads` 区分 correlated=true 的采样内容和启动阶段 correlated=false 的 4 条内容。`braid.provider.turn.elapsed` 为 s 单位 histogram，16:04:54 的 completed/Pi 数据点 count=1、sum/min/max=85.58862175，界面显示原始分布而不跨批次求和。`braid.runtime.transitions` 的 generation_started 文字明确注明不是运行停止证明。证据 `live-provider-metrics.jpg/.txt`；指标事实不判定应用成功。

16:17 内建第一现场终态回读：Console 当前 supervisor 为 **blocked，不是 stopped**，Generation 仍为 `01a129ff-19d0-73a3-93e2-b6ae52a9e24c`。完整错误保留两条 session 的 `shutdown did not confirm all executions stopped: session reception is unknown: provider request Pi child exit after stop timed out`；health.ready=false。此轮停止资格未通过，不能用 Turn interrupted、grant 撤销或之后找不到进程追认实际停止成功。浏览器没有发出停止或重启命令。

Collector 在 blocked 状态仍可读：3084 条、342 批次、接收错误/重复批次均为 0，原实例与接收历史保留。证据 `blocked-stop-overview.jpg/.txt` 展开了 supervisor 原始状态及错误。后续修复重跑须保留此现场身份；等待根或旅程 Owner 的阶段交接与新隔离现场，当前不宣称本轮端到端验收完成。

根决定保留第一现场 blocked，不追认 stopped；下一次采用全新 attempt2 source、Git 对象库、平台、Harness ledger 与原生 home，Console 端口 19318、平台端口 19282。旧页面已关闭，证据保留；后续浏览器资格单独记录 attempt2，不复用第一轮业务实现结论。最终停止须对应 supervisor 与实际 Pi PID/session/ExitStatus/generation 事件，SDK teardown 仅作为采集收尾证据。

## 内建独立 attempt2

16:25 浏览器打开独立 Console `http://127.0.0.1:19318/`，平台 `http://127.0.0.1:19282/`，Generation `01a12a0e-48bb-7151-98ea-720f322493c6`。Supervisor running、Ready 是、Provider 启用；Collector 1486 条/130 批次、接收错误/重复均 0。资源实例明确是 `real-e2e-builtin-minimax-attempt2`，不混作旧现场；服务仍为 `braid-real-e2e-builtin`。原件保存在 `/Volumes/WorkSSD/Development/braid-e2e/builtin-attempt2/browser/`。

PR #2「实现离线笔记 CLI（Issue #1）」由 issue-minimax 创建、指派 pr-minimax、revision=2、open，候选尚未发布。Head `ae8c303c4519994eeac67da6b4036fe2c8f1f2df`、Base `fb1449e3321faa6494472f42a7e6fe2b09aebc37`；正文明确关联 Issue #1。独立 Harness 账本的 upstream cursor=8，space_id=`01a12a0e-24e4-7881-99db-39ca1de1b190`，Issue Turn 已 completed、PR Turn running。

按新 instance + PR 原生 session + Turn 精确筛选，记录 #1043 的 get_state 请求标出 Provider pi、PR #2、Assignment `01a12a10-d529-7b63-8240-79f035eaa6e8`、上述 Generation、Turn `01a12a10-d73c-76a0-b9d1-0388b78a1a39`、Trace `c691a2045883120fc7d511d290cd9d28`，与账本对应；correlated=true、sampled=true。只展开这一条属性，未通读 rollout。

证据 `ready-overview.jpg/.txt`、`bootstrap-pr.jpg/.txt`、`pr-turn-session.jpg`、`pr-turn-attributes.txt`。尚未回读业务候选/最终真实停止，仍不判定完整旅程通过。

16:31 停止历史回读时，Owner 已完成首代 stop 并 restart。当前 supervisor 为新 Generation `01a12a15-11d7-7d93-a5f3-38dc9e3082f7` running、Ready 是，Collector 3234 条/244 批次、错误及重复 0。因此没有把浏览器当前 running 记录成 stopped。定向 Trace `c691a2045883120fc7d511d290cd9d28` 已收到 PR Session Span #2639（batch205），对应首代 Generation、PR Assignment 与原生 Session；16:28:24 的生命周期事件明确 `stop / confirmed`、error.reason 为空，Session Span 时长 195432.94 ms。证据 `first-generation-stop-trace.jpg/.txt`。

运行 Owner 的 `/Volumes/WorkSSD/Development/braid-e2e/builtin-attempt2/raw/waiting-stop-child-exit-otlp.json` 原件经定向读取：首代记录 #2624/#2625 分别为 PR PID97663、Issue PID92107，`operation=wait / stage=graceful_eof / outcome=exited / exit status: 0 / code=0`，Session 与 Generation 都可对应。idle 停止的 trace/span 为空，按 Session + Generation 关联，不伪造 Turn。此证据仅说明这两个 Pi 子进程实际 wait 退出；原 body 仍提醒 detached tools 不能由父进程退出推断。Owner 保存的停止回执 request_id `b5c775b5-6d46-4437-a9c9-eb634f862ad7` 终态为 stopped；最终旅程停止尚未发生。

16:32 PR2 revision9 显示已发布首业务候选 `1af808614e0112274ac7eb0baff3d1839599264e`，Base 未变。平台讨论 #4/#7 可读，作者自述执行 `/tmp/notes-evidence/*`，已向根报告物理存储边界疑点，由运行负责人定向核实真实工具原件；Console 没有执行这些命令。只读 Diff 当时已经包含后续 `list --all` 与坏结构 `[]` 拒绝变更，但页面仍展示旧 Head。该异步发布竞态暴露了 Diff 响应缺少自身 OID/revision 的问题，已向根与平台 Owner 反馈；`first-published-pr-diff.jpg/.txt` 保留原始展示，不能将此 Diff 归属旧 `1af8086`。待响应携带实际候选身份后由 Console Owner 修复并实际回读，不重复应用 CLI 验收。

根采用独立判断后授权修复 Diff 显示身份。Console 仅修改 `src/console/web/console.js`，和平台 Owner 确认响应 `{number,revision,headCommit,baseCommit,diff}`：UI 将固定身份与内容一起呈现；详情不匹配时明确过期并可刷新，合并仍采用原详情 Revision；对象选择、路由 Generation 与请求序号拒绝迟到结果；加载/错误清除上一成功展示并保留具体原因。`node --check` 通过，无新增测试。平台冻结读取及整体编译部署由 runtime Owner 负责；本条是源码事实，尚未宣称实际部署或浏览器复验完成。

16:40 修复实际部署后，浏览器 reload 获取新版 UI，PR 详情 Revision14、Head `d639ff30d6a079fb5ede8dbd8a884feeaca3a4c0`，Base `fb1449e3321faa6494472f42a7e6fe2b09aebc37`。实际 Diff 响应的固定快照面板同时展示 number2/revision14/上述 OID 与内容，并声明与详情一致。运行 Owner 正常 start Harness 后发收口评论，PR Revision15；浏览器保持旧 Revision14 详情不刷新，再读 Diff，面板准确展示 Revision15 与 OID，并明确「对象详情已过期。此 Diff 属于以上固定快照；合并操作仍采用页面原 Revision 前提」，提供刷新详情入口。不是模拟响应或测试 fixture；Console 仍只读。证据 `diff-snapshot-current.jpg/.txt`、`diff-snapshot-stale.jpg/.txt`；旧错误展示继续保留。请求失败/迟到分支由代码边界处理，本次未人为故障注入。

16:50 新 binary `b530f0a47ebc1e490979be8036b272d5859fd60fd32374ce50f435b2bd44762a` 部署后，只读 PR2 为 closed / Revision21，详情依据持久合并状态显示 merged。候选 Head `d639ff30d6a079fb5ede8dbd8a884feeaca3a4c0` 与 Base `fb1449e3321faa6494472f42a7e6fe2b09aebc37` 保留；重开、发布候选及合并按钮均不存在，静态说明操作已结束。Diff 仍可读取，固定快照 Revision21/Head/Base 与详情一致；持久合并记录 lifecycle=applied、error=null、merge_commit=`77c8f50a5323756dc85d7a8e7f2dde2d91526c6f`。证据 `merged-pr.jpg/.txt`。这证明平台合并历史和 Console 展示，不替代根的固定 OID 应用验收；自然 finalization 的第一次读取拒绝及后续修复事实由运行 Owner 保留，不用此次 merged 展示覆盖它。最终停止/Collector留存尚待回读。

17:02 内建最终 Console 回读：权威 supervisor stopped、error=null、health.last_error=null，Generation `01a12a31-0bb0-7d11-b1d0-459dfffadc5f`（已退休对象之后正常 restart 的一代）；Provider 字段保留 connected，不以该历史字段覆盖权威 stopped。Collector 11193 条/898 批次、接收错误及重复 0，仍可读取实例从16:22:22开始的历史。平台 Issue1 closed Revision9、PR2 closed/merged Revision22；停止后真实 Diff 固定 Revision22 与上述 Head/Base 匹配，持久合并记录仍 applied/error=null。首代 Trace `c691a2045883120fc7d511d290cd9d28` 的 Turn Span #2619、Session Span #2639 仍可读。证据 `final-stopped-overview.jpg/.txt`、`final-merged-pr.jpg/.txt`、`final-retained-trace.jpg/.txt`。

运行 Owner 另保存 merged 一代 `01a12a2b-a7cf…` 实际 Pi wait #11001（Issue PID33110）和 #11154（PR PID33111），均 graceful_eof/exited/exit0，原件 `raw/final-stop-platform-collector-retained.json`。最后退休后讨论及 quiet/restart 不产生新 PR Turn 的账本验证归 Owner，不用 Console 计数替代。内建浏览器资格完成，无在途 mutation；已交根和运行 Owner 收尾服务。接下来 Console Owner 持续接 GitHub 独立现场资格，不复用内建结论。

## GitHub 独立现场

17:05 浏览器打开 Console `http://127.0.0.1:24318/`，数据与内建分离。权威 supervisor running/Ready=true/provider=connected/error=null，Generation `01a12a34-bfdf-7cf2-aa33-0b060ce10573`，repository=`xiaoland/braid-poc-test`、ingress=`127.0.0.1:18180`、reconciliation=current。资源 instance=`real-e2e-github-minimax`、service=`braid-real-e2e-github`，Collector9条/4批次/接收错误0/重复0。尚无模型输入，Harness snapshot sessions/turns/assignments/worktrees/upstream为空，不推断生成完成。

Console 提供相同 Harness 启动/停止界面，浏览器只读没有点击控制；未声明内建平台，概览及协作页明确说明不提供 GitHub 对象编辑，不能用内建对象UI冒充GitHub写入。证据在 `/Volumes/WorkSSD/Development/braid-e2e/github/browser/initial-ready-overview.jpg/.txt`、`object-control-boundary.jpg/.txt`。等待运行 Owner 真实输入、Session/Turn、业务候选和最终停止节点；公网入口与GitHub App临时配置资格归网络 Owner。

GitHub 首现场由根判定未完成输入资格：实际 GitHub Issue37/comment 官方投递曾因 App URL 漏 `/webhook` 404；修正后，网络人为签名 ping 先用假 repository.id=1234/node 配真实 full_name，污染原 Harness ledger，使后续官方真实 node 与身份唯一约束冲突503。签名 ping202仅证明网络路径，不能替代官方投递。根与 Owner 明确此为验收输入污染，不改产品身份约束、不私删状态；此现场 sessions/turns=0，正常停止后转全新 GitHub attempt2 source/ledger/native/Collector，同空白baseline继续Issue37。浏览器原24318初始空账本与Ready画面保留 failed 输入资格身份，503原件归 `github/network`；不把新现场成功覆盖首次失败。

GitHub attempt2初始浏览器17:15回读：Console25318，独立路径 `/Volumes/WorkSSD/Development/braid-e2e/github-attempt2/browser/`，instance `real-e2e-github-minimax-attempt2`、service `braid-real-e2e-github`，Generation `01a12a3e-48a9-7b90-8ba1-8dc4f456b89b` running/Readytrue、Collector27条/7批次。17:15:33 Harness snapshot已出现真实 Issue node `I_kwDOUCYt2M8AAAABWa_jkA`，Assignment `01a12a3e-c5c4-7ad1-aa03-19a9e05d4d21` lifecycle=blocked、17:15:19退休，队列runnable/urgent，Session/Turn仍为空。已向Owner反馈，等待其具体原因处理，不由Ready推导模型执行或输入成功。证据 `initial-ready-overview.jpg/.txt`，与首次污染现场独立保存。

17:19 运行Owner修正GitHub g1工作目录fixture并保留blocked后，实际 g2 Assignment `01a12a40-8aeb-7d82-abb8-12cdfe60762f` active，原生 Session `2026-10-11T09-17-17-748Z_01a12a40-9434-7223-83ac-72bb9c356155.jsonl`（Issue37 WorkSSD worktree）running，Turn `01a12a40-9489-7742-8359-537e7b1b4933` running、trigger_kind=trusted_mention，queue consumed。新 Generation `01a12a40-6e34-7ac1-ac6d-071448487ac6`。

精确 instance+Session+Turn 过滤后，单条 get_state 请求日志 #93（batch19）显示 Issue37、Assignment generation2、上述 Runtime Generation、Turn 与 Session，Trace=`5da1abe0ec6bfbb118d6fb90ab3cd00c`，Provider pi/correlatedtrue/sampletrue。原生 provider.turn.id空值保留，不虚构。只展开这一条属性，没有通读rollout。证据 `issue37-running.jpg/.txt`、`issue37-turn-session.jpg`、`issue37-turn-attributes.txt`；这说明真实模型Session关联，不提前判定PR/应用或终态完成。

17:25 真实 GitHub Draft PR38 经实际事件激活：Harness node `PR_kwDOUCYt2M8AAAABHy1Epw` 对应 active Assignment `01a12a45-9e88-72f2-ab46-c647b97eccdb`（generation1）、Pi Session `2026-10-11T09-22-50-183Z_01a12a45-a6c7-76e3-bb13-8e4f9a0d6b46.jsonl`、Turn `01a12a45-a704-7e91-a362-90de0e9df458` running。Issue37原Session idle、其首次Turn completed，两个对象的原生Session/Worktree独立。浏览器精确过滤后单条 get_state #3317属性明确 pr/38、上述 Assignment、Runtime Generation `01a12a40-6e34-7ac1-ac6d-071448487ac6`、Session/Turn，Trace=`c68e2d6a0e94037f937224965f40dfbd`，sampled/correlatedtrue。未伪造初始化空providerturn关联，未通读rollout。证据 `pr38-running.jpg/.txt`、`pr38-turn-session.jpg`、`pr38-turn-attributes.txt`。官方PR与候选OID/真实Diff资格归运行Owner和根；Console没有GitHub对象编辑，不用内建对象页代替该事实。

17:32 config 继承缺陷修复前的公开停止，浏览器实际 supervisor stopped/error=null/health.last_error=null，Generation `01a12a40-6e34-7ac1-ac6d-071448487ac6`，Collector9753条/628批次、错误及重复0。证据 `config-fix-stopped-overview.jpg/.txt`。运行Owner原件 `raw/config-inheritance-stop-confirmation.json` 的 Pi wait #9746/#9747 对应同代 Issue PID56768/PR PID62548，graceful_eof/exited/ExitStatus0。Owner指出 GitHub 未继承 BRAID_CONFIG，导致 Agent 发布排查误用旧配置；由根修共享 Config 并保留旧错误/候选原件。这是中途停止事实，尚非最终旅程成功，浏览器未操作重启。

17:56 GitHub 最终独立浏览器回读：25318权威 supervisor stopped、error=null、health.last_error=null，Generation `01a12a62-3dff-7890-baff-8c7a25895d4a`（退休后正常 restart 验证代次）；Collector15357条/1146批次、错误及重复0，记录从17:14:47起保留。Harness PR38 Assignment、Worktree与最新Session为retired，旧Session replaced；Issue37 Assignment sleeping。官方Issue closed/PR merge事实由GitHub原件负责，Console未声明为内建对象权威。首个PR Trace `c68e2d6a0e94037f937224965f40dfbd` 的3个真实Turn Span和1个Session Span仍可读，具体Span #9052/#9731/#9732/#9734。证据 `final-stopped-overview.jpg/.txt`、`final-retained-pr-trace.jpg/.txt`。

运行Owner `raw/final-runtime-stopped-collector.json` 原件定向读取的末两实际Pi wait：#15173 PR PID93319、原生09:48:41 Session；#14557 Issue PID83683、原生09:39:06 Session，均 generation `01a12a53-c814-7832-ab86-0eef39c07ae7`、wait/graceful_eof/exited/code0。它们是之前模型代次的真实关闭事实，不能伪配最后无Session的退休验证代次62。postmerged讨论/quiet/restart不增加PR Turn或再次finalization的时序证明归Owner前后账本；此浏览器snapshot共13 Turn（Issue与PR合计），没有由总数单独推导幂等结论。GitHub Console资格完成，初始污染failed、g1fixtureblocked和Config继承故障均保留，未被最终结果覆盖。没有在途浏览器mutation，已通知根和Owner可以服务收尾。
