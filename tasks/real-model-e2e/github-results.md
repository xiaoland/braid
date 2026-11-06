# GitHub 上游真实模型执行记录

2026-10-11 内建实际流程结束后接入。独立source从6425f01开始，新需求基线2cc02e78920072fdf1165def8798464012630655只包含允许可见需求、README占位和忽略规则，不继承内建业务答案。基线按批准计划正常push main。运行使用最终同版binary81f963d0d5b7b4597e3707527deb0e01c4775f99197e7be242b82e84a556ccd9，GitHub配置SHA47eac7bf65b8d5b64b545b5282cb6a777ed75379665b204f54c24f2e2da4ed58；双Profile均官方MiniMax-M3，quiet30/count8/reconcile60，实际原生指令明确所有数据/材料/临时/缓存必须本轮WorkSSD、禁/tmp。

为了保留旧attempt1 blocked/unknown现场14318，GitHub独立Console与OTLP使用24318；ingress18180和health18181保持网络合同。新Harness账本0→6迁移完成，未复用旧native/store。Console真实running/providerconnected/readytrue/reconciliationcurrent，health18181/healthz实际200，tunneldisabled，因为公网入口由稳定networkowner负责。初始历史对象激活情况见raw/initial-no-historical-activation.json，未创建本轮Issue或模型输入。startup-identity.json、first-start.json、first-ready.json在/Volumes/WorkSSD/Development/braid-e2e/github/raw。

相对config路径首次migrate被公开CLI拒绝，改完整绝对路径后实际迁移成功，原工具错误保留；不属于产品回归。网络App切换和恢复由e2e_readiness owner负责，公网签名入口未完成前不创建任务激活。


网络owner已实际完成公网签名ping202/ledger消费及App仅URL切换readback；该请求不是GitHub实际发出的对象事件。随后正常operator创建GitHub Issue37，title唯一20261011-minimax-peer-upstreams，正文包含允许需求/隐藏HTML判据及实体WorkSSD环境约束。普通maintainer可信@braid评论6107427719明确Issue设计only，再由braid gh pr ensure交接PR，不代写业务。创建前openIssue0。创建/评论实际返回URL，原件create-issue.json、trusted-issue-activation.json保留。

跨quiet30后Harness只有先前signed ping delivery，没有issues/issue_comment delivery，provider sessions/turns仍0；Console无业务错误。这个是事件通路未证成，不当模型零结果或已激活成功。未重复评论/注入事件，已将真实Appdelivery请求/响应及公网入口问题交稳定networkowner有界调查。原件github-activation-after-quiet.json保留；首次证据序列化bytes错误原工具保留，最终原payload按base64保存不丢失。


网络首次官方Issue37/opened与comment6107427719投递HTTP404：临时AppURL缺/webhook，网络owner仅修URL路径/readback，不改secret。正常唯一澄清comment6107449747再投递HTTP503，具体SQLite UNIQUE repositories.name_with_owner。Owner定向确认是先前signed验证payload虚构node MDEwOlJlcG9zaXRvcnkxMjM0却用了真实repo名，真实Githubrepo1344679384/node R_kgDOUCYt2A，因此原账本权威冲突；不修产品接受错误身份、不删row。原repo/payload/真实API事实github/raw/repository-unique-conflict-identity.json保留。

按根与advisor采用正常stop旧污染Console，零sessions/turns/assignments/workitems且stopped/errornull、Console正常退出。保留原firstfailure；新github-attempt2独立source/objectlib/runtime/nativehome/collector使用同无答案baseline2cc02e7，现Issue37继续，没有新Issue或signedpayload，保留正确AppURL/tunnel18180。新Console25318、health18181。首次正常comment6107488372官方delivery44841e70-c554-11f1-8a4f-f39c6fa9ccca入库成功、真实repo R_kgDOUCYt2A和Issue节点关联，和network人工验证分别取证。

首次materialize被我fixture路径替换错误阻断：双workspace重复/Volumes/WorkSSD/Development前缀，model0sessions/turns。原配置config-first-path-error.toml、blockedledger first-official-ingest-blocked.json保存，不修产品容错、不改blocked私有状态。精确修正两完整workspace为github-attempt2/source，normalstop/restart后新正常trustedcomment6107503398激活g2；旧g1blocked错误保留。最终configSHA08127d23c71caa2c7f253680fa33164bc267aff47abbd68dc4f18effc1a1170d，公开configcheck成功，全部source/home/state/worktrees/backups/OTLPdb实际路径resolve存在且df为/dev/disk6s1 WorkSSD，actual-resolved-config-paths.json保存。

真实IssueAgentturn01a12a40-9489-7742-8359-537e7b1b4933开始，原生Pi文件github-attempt2/worktrees/issue-37/issue-minimax-g2/.braid/pi-sessions/2026-10-11T09-17-17-748Z_01a12a40-9434-7223-83ac-72bb9c356155.jsonl，首userf4799623。仅assistantmetadata核对provider=minimax/modelMiniMax-M3，已有真实bash/toolResult收发，未展开assistant/CoT。后续自主PRensure/业务产物在途，不能仅由ready/ingest/原生响应称黄金路径通过。


Issue Agent实际braid gh pr ensure建立Draft PR38，bootstrap8cf6782a4792d5e571d9aa708d43cacc11b6362c；PR独立原生turn01a12a45-a704-7e91-a362-90de0e9df458真实实现后09:24:53提交、09:24:57 push首业务候选e08485391b252892fec16f6678fb6084e0de9dbf。父根负责该固定OID独立应用验收，未merge。

09:25:12实际原生工具echo证实BRAID_CONFIG/BRAID_INSTANCE为空；src/config.rs585/602仅Builtin注入braid_config_path，GitHub缺失精确配置继承。模型为发布评论从09:25:13开始误用旧builtin-attempt2配置，并实际直接SQLite UPDATE旧Builtin repositories.name_with_owner为xiaoland/braid-poc-test，随后改回braid-poc-test。当前readback恢复原值，但不能宣称旧ledger未触及。首候选提交/push早于该跨空间排查，定向生成前未发现旧业务答案读取；后续独立性/内部状态违规保留，不能抵销。raw/config-inheritance-cross-space-failure.json保存空env/原UPDATE/当前readback。正常operator评论6107592250明确正确绝对Github config、限制当前空间、禁止直接内部数据库读写及其他run。根负责共享config注入修复，原生仍正常运行不伪停/不私改状态。


配置缺失执行通过公开Console stop确认stopped/errornull；真实Pi childwait Collector9746/9747分别IssuePID56768、PRPID62548，graceful_eof/exited/ExitStatus0。当前turncompleted或interrupted按原ledger分别保留，拒把停止受理当停止完成。root修config两个provider统一精确loaded_from，唯一中央strictClippy/build通过后Consoleexit0、atomicinstall新版SHA728df182e9461e9150b8dee3c3bd2bbfd86989c9d08b58c457e441c8a6067e62。同ledger/newgeneration正常接续，实际原生tool recorda9b840e3回读正确Githubconfig/BRAID_AGENT1。正常Appcomment身份以官方原件为准；仅以实际CLI与origin_echo判断，不伪称所有隔离已通过。

root固定e084独立9次原始应用CLI全部通过。正常Issue正文接受list --all/同标题唯一ID；第一编辑实际PR已idle（初先前running摘要已更正，不算active），两对象contextresetactive_turn_idnull/applied。随后PR正常讨论6107657654/6107657851合并event_count2，quietdeadline09:38:15，quietpending时公开stop确认+正常restart，09:38:20仅同batch一个turn开始。其实际running时再Issue正文澄清all状态active|archived；09:39:32旧turninterrupted、resetactive_turn_id指向原turn/applied，新native09:39:38+新turn继续。raw/quiet-pending*.json、active-body-reset-observed.json保留。


新设计候选08c2458751a02a8143e0cb7fe620c20bd48dea16真实push后root独立10次CLI全部通过。两个session均idle/latestnativecompleted且outbox9applied时，由稳定networkOwner停止隧道；本地ready/reconcile保持，公网530/Cloudflare1033事实由networkOwner保留。唯一正常PRcomment6107701629官方delivery真实530，AppURL/secret未改/不redelivery。09:44:28实际reconcile-2020318698c485da3aadce3bb41e689a记录对象IC_kwDOUCYt2M8AAAABbAwhfQ，batchquiet09:44:58后09:44:59单turn触发。actualusera9762346引用该comment、toolResult3113a68b实际读取完整正文，09:45:17completed无业务变化。raw/tunnel-down-*保留ingest→batch→turn→native因果，不以一次canonical回读冒称事件接续。普通wake事件表仍pending而batchconsumed的现有投影分别记录，不改ledger或造新事实。网络恢复交根followup原networkOwner，模型执行仍同官方M3。


## 最终运行与矩阵结果

本轮最终实际源码为 `3fab0d0`，binary SHA-256 `728df182e9461e9150b8dee3c3bd2bbfd86989c9d08b58c457e441c8a6067e62`；配置仍 `08127d23c71caa2c7f253680fa33164bc267aff47abbd68dc4f18effc1a1170d`。发布演进为无答案基线 `2cc02e7` → 空bootstrap `8cf6782` → 首业务 `e084853` → 设计业务 `08c2458` → 正常merge `8281fffa0bb76398a0cd4c262bd69351e9ed2da6`。固定候选的独立应用操作及merge树/parent资格归 `qualification-results.md`，不由生成Agent的评论替代。

| 事项 | 结果与限制 |
| --- | --- |
| 真实官方事件与双角色模型 | pass：真正Issue/评论/PR节点、Appensure交接、官方MiniMax-M3实际原生收发。首404/503与fixture路径blocked保留，不计首错通过。 |
| 黄金路径与设计变化 | pass：首业务9次与新设计10次独立真实CLI。新OID由Agent实际提交/push，开发Agent不代写业务。 |
| canonical Context | pass限定首user/新设计user与明确canonical读取；隐藏HTML不进入这些投影。raw GitHub对象读取可含原始隐藏正文，不声称全面信息隔离。 |
| quiet、重启与自身回写 | pass：两普通讨论event_count2/quiet30，pending时公开stop/restart，同batch单turn；App自身评论origin_echo consumed不形成自身评论循环。Git push synchronize是真正external事件，另开收口turn单列。 |
| idle/active正文更新 | pass：idle双对象reset；active关联设计reset明确旧turn interrupted、新session/applied，无并行旧新turn。 |
| 生命周期 | pass：普通PR close实际finalization completed→sleeping；reopen原责任激活并新native；merge不打断已运行turn，待其completed才单次finalization；最终PR assignment/agent/session/worktree均retired，自动Closes Issue37收口sleeping。 |
| 停止与退休后重启 | pass：真实Pi wait/graceful_eof/ExitStatus0；postmerged comment6107768185、quiet30、normalstop/restart再quiet30，PRturn总数保持11，没有第二mergefinalization或重新激活。 |
| 隧道中断/reconcile | pass：真实官方comment HTTP530；reconcile ingest→batch→turn→实际原生comment引用与正文读取→completed。公网恢复产生新host的App URL变化/最终原URL恢复归network Owner原件；不redeliver、不synthetic。 |
| 普通App native改派与Builtin writer grant | unavailable/not-applicable：普通App trusted mention模式不能宣称native unassign/reassign，Github未模拟Builtin grant能力。Builtin责任fencing见内建实际结果。 |
| 全程隔离/操作合规 | fail：缺配置继承导致真实模型误用旧Builtin空间并直接UPDATE旧ledger，虽已改回原值但不认未触；保留原工具与修复/正常接续，不由应用通过抵销。 |
| OTLP/Console | pass限定本轮真实对象/原生session/turn/停止关联及停止后历史可读，最终15357条记录。GitHub对象权威来自GitHub，不宣称Console有GitHub对象编辑界面。 |

最终公开stop请求 `25ef7b28-48af-4931-b512-f5035d46d5c5` 实际stopped/errornull，generation `01a12a62-3dff-7890-baff-8c7a25895d4a`。此前实际模型generation `01a12a53-c814-7832-ab86-0eef39c07ae7` 的最终Issue/PR native wait记录14557/15173，PID83683/93319，均graceful_eof/exited/code0；末次退休后启动未重新生成原生会话。`raw/final-runtime-stopped-collector.json`、`final-no-reactivation-stop-request.json`保留。Console25318完成最终独立浏览器回读后正常退出0；旧Builtin14318进程退出1并保留具体stop unconfirmed错误，不能追认原provider stopped。所有临时监听已关闭，原DB/rollout保留，原件final-console-process-exit.json。network Owner已独立恢复原App URL并读回一致、关闭隧道，见github-network-results.md。

原生SDK usage按responseId去重、仅提取metadata：GitHub attempt2共358个assistant响应，非缓存input658424、output83872、cacheRead11237691、totalTokens11979987。按冻结RMB/M token input2.10/output8.40/cache-read0.42估算人民币6.80704542；账号实际扣费未知，native cost0不当免费。原件 `github-attempt2/raw/native-usage-aggregate.json` 保留6个Pi原生文件分项；它们不是6个不同Braid责任角色。


最终浏览器只读验收完成后，GitHub25318 Console进程PTY50397正常Ctrl-C退出0，Collector原件保留。旧Builtin14318 Console进程PTY32355正常结束退出1，仍报告原runtime stop unconfirmed/Pi child exit timed out；这是服务器进程已关闭，不能将旧native未知追认停止成功。定向实际ps无本任务Braid/Pi/cloudflared/Wrangler，14318/19318/24318/25318/18180/18181/18282/19282均无监听（connect_ex61），包含临时平台server。raw/final-console-process-exit.json记录退出/错误/端口事实，不删除旧ledger/rollout。最终App原配置恢复与隧道关闭须采用network Owner回执，本Owner不从本地无进程推断远端App已恢复。
