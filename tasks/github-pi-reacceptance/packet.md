# Braid 真实模型验收

当前状态：2026-10-12，真实 MiniMax `braid run` 的 ARC-bench lite 两题验收及收尾已完成，结论不通过。Keep 程序 completed/停止已确认，公开 Lite 本地复现6/32；BookStack blocked/停止已确认，失败生成快照2/34。合计8/66（12.12%）。两题均只有root真实执行，没有成员分工；另有/tmp写入和按名称停止请求的纪律失败。官网两次HTTP403未创建submission/run，不存在官网分数。

完整事实见 [run-arc-lite.md](run-arc-lite.md)，独立评分原件位于 `/Volumes/WorkSSD/Development/braid-run-arc-lite-20261012/evaluation/`。本轮全部进程、应用服务及远端评估容器已退出，相关端口关闭；无在途操作，不自动重新生成、补发布/合并或按评测反馈修应用。已修复有限指派吞输入、登录shell阻塞及共享ledger写事务；SQLite修复仅有编译和公开绑定操作反馈，没有进入本轮模型配方。

下一步由用户决定root职责与交接、执行纪律及应用交互的整改范围。本packet仍保留Braid反哺的未完成资格入口；原GitHub+Pi第二候选应用复验通过，但剩余生命周期资格仍未完成。以下保留原范围与历史事实，不启动旧GitHub运行。

原 GitHub+Pi 授权：用户指示：“好的，现在继续重新验收，先仅验收 GitHub + Pi；全部用 minimax 模型。”沿用已知 `xiaoland/braid-poc-test` 和官方 MiniMax `minimax-m3`（wire MiniMax-M3），凭据只从 ../agentic26/.env 加载，不输出或复制。真实官方按量调用获授权，无人为费用停止线，无备用模型。

目标：判断反哺后的独立 Braid 能否在 GitHub 中完成真实 Issue→PR 实现与更新，并在真实 Pi/subagents/background 运行下交接结果及停止，不以编译替代。候选源码 `1394319`，冻结binary与锁定Pi1.0.2材料SHA；只启用subagents/background，不装可选网络工具。所有根／子模型必须 minimax/MiniMax-M3，显式角色材料与模型路由禁止ambient fallback。

范围：新的需求标识 `20261011-github-pi-native-v2`，应用目录 e2e-notes/20261011-github-pi-native-v2/。新的source/ledger/worktree/native/OTLP/Console位于 /Volumes/WorkSSD/Development/braid-gh-pi-reacceptance。创建专用Issue/评论/PR/分支、需求空白基线、实际push与明确采纳合并；临时App webhook变更先保存原始配置，结束恢复并真实核对，不写掩码secret。不接管旧对象，不改其它仓库或旧运行状态，不运行Factory/内建/赛事/设施测试。所有Mac缓存、HOME、临时、SDK材料与工具写入都在WorkSSD。

主Agent维护判据、版本采用与独立应用实际操作；native_local_env稳定拥有本轮配置、真实GitHub网络、Console/Harness控制、模型运行及过程诊断/必要修复/恢复，结果写run.md。原Pi plugin/ownership owner处理其模块的必要原生修复，中央Cargo由主Agent负责。仅使用内部agents工具，绝不联系独立Codex聊天；所有owner不得代写业务应用、改私有ledger或注入伪事件。

完成判据：真实官方事件→Issue当前Context→两角色实际MiniMax生成；PR专属worktree真实diff及发布；关联设计变更落到实现；普通讨论quiet/coalesce与自身echo不循环；空闲／活动Context reset、关闭／重开／合并终态；公开停止、原生exactowned后代清理、重启不重复已完成输入；真实子Agent至少同步及异步各一次，PBB有限结果／service分别取证，保留错误／取消／自然重试发生与否，不合成HTTP错误来制造retry；sample=1.0的OTLP/Console保留对象/session/turn/usage/错误，停止后历史仍可读。应用独立固定commit实际操作符合需求，开发Agent不写答案。

基础需求沿用笔记CLI：add标题正文返回稳定唯一ID，list按创建顺序只列未归档，archive幂等且原正文保留；中文、引号、换行、跨进程持久化、无效ID／损坏JSON具体非零错误且不覆盖数据。关联设计变更新增重复标题与list --all状态。实现角色用真实subagent协作和PBB后台应用操作；有限义务结束后才交付。另通过正常协作消息安排实际活动停止与恢复，服务与有限任务的owned停止分别取证。

每项pass/fail/unavailable/not-run明确区分，无纠正黄金路径单列，设施修复保留原失败与新版本；不放宽验收判据。普通GitHub App trustedmention能力按实际模式声明，不冒称nativeassignment。未发生自然retry即not-run，不把其余成功宣称retry已验。此轮为GitHub/Pi源码候选资格，不宣称taggedrelease、Codex、内建或三次完整campaign。

已冻结主binary SHA-256 `f015b77bdc03ee44f42dcec6c62879abd04f72e2b68254d6a37725a977fcac95`，源码完整身份 `13943195dfb9b162e99fda6187fd0f238d0afdae`，WorkSSD可用493GiB。初始与设计变更正文位于input/，本轮业务目录基线为空；仓库其它历史应用不属于允许输入，角色禁止读取／复用，不声称GitHub仓库整树为空。

下一步：owner只读核对现有权限、原App入口与无活动运行，准备准确新路径、模型材料与模型唯一性，记录冻结身份后启动Console/tunnel/Harness并创建真实需求。主Agent独立采用产物和证据，收尾恢复外部入口、停止进程、汇总消耗与剩余边界。

准备现场：初次 selected subagents/background manifest `4a87c7d96b0b0fe59cf53fc78815f23c96dd0aaf9b3d48f730e3d477030fdc9b` 字节核验通过，但实际 doctor 暴露复制 Homebrew Node 的 `@rpath/libnode.147.dylib` 缺失，未进入付费模型阶段。pi_plugin_feedback owner 修安装器的独立 Node 交付，保留原安装及 dyld 错误；新目录 pi-attempt2 完成真实启动后重新冻结。公开 CA 保存在 WorkSSD，操作脚本保持 TLS 验证。两 profile 与两个显式子角色均固定 minimax/MiniMax-M3，内建角色禁用、fallback 为空，最终实际调用仍需从原生与 OTLP 证据核对。

主 Agent 独立浏览器现场：Helium 中打开 http://127.0.0.1:26318/，Console 显示 stopped、Provider 禁用、Generation 无、Ready 否，已接收记录/批次/资源来源均为 0；GitHub 接入启停与观测可用，未声明内建平台。根只读 UI，runtime commands 由 native_local_env 唯一写。需求空基线为远端 main `4c49fff0a2e5c778c57dc627ef3c2ded06f10d75`，仅本轮 REQUIREMENTS；首次 push 虽成功，原 Keychain helper failedstore -60006 保留，后续采用显式 gh helper。

安装修复已本地提交 `89505d964314cd6f65b928543f33c59536fde386`，正式阶段源码以此冻结。Rust 未改，binary 字节/编译来源保持前述 `f015...` / `1394319`。新 native/pi-attempt2 manifest SHA `d1fd63d831d2753a21c0b22e59df5bddaee0ca3fbc04cf48485f76edff2bb9bc`；安装 owner 实际运行 Node24.15.0 / Pi1.0.2 并核对只依赖系统库，原 Homebrew dyld 失败保留。证据 `raw/native-pi-attempt2.json`，该材料随后仍须真实 doctor 与模型调用。未 push Braid 源码。

正式 Harness 已通过公开 runtime command 接受启动：request `37dfdca1-7ec5-447a-81cb-09ed421c9058`，generation `01a12b2e-9f58-7eb3-8ead-8831089c8657`。根独立浏览器看到 running/Ready 是、采样 1、同 generation，Collector 9 records/3 batches/接收错误 0，尚无 Session 或原生内容；这是服务启动证据，不是模型成功。实际 doctor 各项 Pass 只证明版本/材料/配置，不冒称完整 live provider 能力。GitHub App selected repositories 已实际只读核对二项，owner 接着正常创建 Issue 及 trusted mention。

本轮真实 Issue 为 https://github.com/xiaoland/braid-poc-test/issues/39，创建于 2026-10-11T13:38:21Z。临时 App webhook PATCH 与 readback HTTP200，目标含 /webhook，原始配置仍保留；opened 的实际 delivery 由网络 owner 接续核对。

付费前实际原生配置读取结束：`raw/subagent-model-config-fixed2.json` 的 rootSDKModel 与两个角色 resolved/candidates 均为唯一 minimax/MiniMax-M3，fallbackModels=null，六个内建角色 disabled，extension/runtime errors 为空；该无推理操作 exit0/owned cleanup stopped。user frontmatter 的 [] 与 false 都曾被轻量 scalar-list parser 当候选字符串，原错误原件保留，最终删除该字段走 primary-only 路径。此证据不替代正式根／子调用模型元数据。owner 已开始通过真实 Issue39 trusted comment 触发付费流程。

正式真实调用：trusted comment https://github.com/xiaoland/braid-poc-test/issues/39#issuecomment-6109679664 于 13:46:05Z 接受；Issue assignment active，provider session `2026-10-11T13-46-23-912Z_01a12b36-f328-759e-aff8-47bf69ad3b2d`，Braid session `01a12b36-fe19-7c82-af4c-932a82bf9737`，turn `01a12b36-fe1d-76d2-8436-92e374d3d815` 13:46:27Z running。owner 定向实际 model_change 与五条已返回 assistant metadata 均 minimax/MiniMax-M3，无模型 error，有实际 token；`raw/issue-first-native-model.json` 保留。SDK cost0 不是费用证明。

实际异步 design child `f6c0cac1...` 完成并由 wait 消费，原生子会话和 attemptedModels 均唯一 M3（`raw/issue-child-model-stage1.json`）。Issue模型发布实施请求 `6109703753` 并正常 ensure 建立 https://github.com/xiaoland/braid-poc-test/pull/40，head braid/implementation-request-6109703753，已附本任务 artifact。首设计曾将 async+wait 误称同步，并提出跳过坏行、created_at/id 排序及附加flag，偏离冻结要求；owner 于13:49:32Z正常公开澄清 `6109710304`，保留原评论，不代写答案。本轮无纠正黄金路径不能计通过，后续纠正后行为单列。

首候选 `33b12109b69c8a9b4ec6c15c602c4e6c51c6d388` 已实际 push。根独立 17 次应用 CLI 操作记录与判定归 application-results.md：持久化/幂等/具体错误拒写通过，合法记录副本的创建顺序边界失败，正常反馈给模型修正；不修改业务代码。运行 owner 定向原生记录另确认模型 cat 到 /tmp/evidence-comment.md、实际 body-file 发布后自己 rm，当前文件不存在；原始 call/result 保留，WorkSSD-only 本轮 fail 不抵销。模型按进程名 kill/pkill 自己的服务也不作为公开 exact-owned stop 通过。后续仍通过公开控制独立验收停止与恢复。

用户最新资源约束："电脑性能好像开始有点问题，你悠着点"。立即降低并发，不新增模型/子任务/关闭重开触发；second candidate `95fa210c8e5692be1af8c89fc8e2f128ff1a8911` 已冻结。根关闭本任务新建 Console/PR 浏览器 tab，唯一性能 ps/vm_stat 采样见本轮工具记录：run bin/braid PID24988约84.5%CPU，另有外部高负载；不据单次采样断定全部原因、不控制其它任务。native owner 按已授权公开 stop 收尾 Harness、保留 owned receipt；根后续应用操作仅顺序小批执行，生命周期下一阶段资源允许后再推进。

降载实际收尾：公开 stop request `c77a14da-e1e1-4bf6-8f38-1845c70ac0ff` 已 applied/state stopped/error null，同原 generation，当前 registered_sessions=0；两个当前execution的exact execution-stop(scope execution)固定coverage均stopped（`raw/final-owned-stop-receipts.json`）。该停止前两个会话idle，不能冒称新一次active stop。App webhook三字段与baseline实际PATCH/GET200一致。stop后唯一ConsolePID24988 CPU采样0.0%/RSS18784KB，前为62.7%/33904KB；控制后旧二execution及reset关闭证据保留，无名称pkill补证。Console/tunnel进程最终结束由owner接着确认。

静态追因定位周期全材料哈希：GroupWorker idle每2s → SessionManager.check → PiSessions.check重复runtime::load约28640files。独立advisor建议去掉周期扫描，无缓存；所有start/resume/懒spawn(325/388/427)仍进入实际启动前runtime::load，doctor完整校验保留。源修复 `d9f52d9` 已本地提交，单任务offline cargo check7.3s和diffcheck通过。正式已结束运行仍使用旧binary；该修复没有部署或重跑CPU，不能把停止后0%归功于新源码。

接续入口：候选95fa固定archive及本轮全部ledger/native/observations原件均保留，配置与材料身份见run.md。恢复前采用readiness修复并以受控单任务方式构建/实际核对；新运行身份和可能恢复输入先冻结。关闭/重开、重启不重复输入、活动公开stop以及最终merge尚未跑；自然限流后恢复已观察，精确retry-intent合同证据不足，不人为制造失败。用户“悠着点”优先于启动更多轮次，当前不新增模型。任务范围未被宣告完整结束，保留此唯一active packet供低负载接续。

最终进程收尾已确认：Console/tunnel正常Ctrl-C exit0，ports26318/18380/18381无LISTEN、knownPID24988/65289/65611均不存在，`raw/final-port-process-checks.json`；无在途操作/新增模型。metadata轻量汇总10物理JSONL（3root+7child、排除transcript、sessionID+entryID去重重复0），root/child actualmodels均唯一M3；recorded totalTokens15193632，input515803/output161755/cacheRead14516074/cacheWrite0/reasoning22833，SDKcost0非免费，账单未取得。自然HTTP429 TPM rate limit(1039)实际出现两次(14:07:52/55)，随后同turncompleted；owner定向核对已存原生JSONL，没有显式retry/willRetry/auto_retry条目；自然429后同turn恢复已观察，精确retry-intent事件合同证据不足，不判完整通过或完全未运行。没有新增探针/模型。

2026-10-12 新增设计前沿：用户要求考虑将 Factory 的 root Issue/description 自主拆分、建 PR 和实现闭环保留为与 serve 并列模式，built-in 仍是同层 minimal GitHub。当前只调查/设计，不改源码、不启动运行。提案见 [autonomous-mode-design.md](autonomous-mode-design.md)，主 Agent拥有边界与采用，内部 autonomous_mode_advisor 只读核对旧运行生命周期。旧验收剩余义务保留，不被新模式提案覆盖。已创建 PR148；本提案尚未提交或推送。下一步收敛有限运行的权限、终态及资源准入合同，交用户评估。

advisor 返回已采用：Factory 旧 local 区分 quiescent/delivery_complete/execution_settled，variant 又检查 root CLOSED；静默不能自动完成或判 blocked。根关闭继续撤销 writer，提前关闭且子项未完返回 blocked，需外部明确重开；父子关系应在创建子项时原子建立，范围不能通过任意链接扩大。基础/最终整合 PR 等固定阶段留给调用方配方，不作为 run 必经路径。提案已同步，当前只有任务材料改动，无源码/运行/远端变更。

2026-10-12 开工：用户明确“是的，开始。”授权上述自主 run 设计的必要源码、文档和验证。当前实现不启动新付费模型，也不改 GitHub 对象。分工：run_platform_owner 拥有 platform 与对象 CLI 的父子关系、范围读取、grant、结果回流；run_runtime_owner 拥有 run 入口、执行身份/范围、有限终态、调度准入及恢复；run_console_owner 拥有 Console 共享控制及展示；主 Agent 拥有跨边界采用、文档、单任务构建与真实无模型平台操作。禁止设施测试/自检，所有产物存 WorkSSD。

当前技术选择：每 run 使用独立 Harness ledger/config 和 run_id，平台仍持有唯一工作图，避免 serve 旧 assignment 在恢复时进入新 run。run 接续复用本 run ledger，不能只过滤新事件却恢复无关旧 session。实施发现总物理 session 限2会因 root+idle子Issue占槽阻断PR；该并发合同由advisor核对原生恢复后收敛，暂不宣称 active-turn 限制等于总物理资源限制。下一步完成平台/运行/Console共享接口，串行编译并以实际对象操作验父子权限及幂等，保留真实模型资格缺口。

实施收敛：统一 active-turn 准入默认2，原描述“实际资源占用”收窄为在途 Braid轮次；不承诺 idle/Pi内部子任务资源上限，不引入自动闲置驱逐。平台注册root绑定run_id、已存在的Harness client_id及delivery_ref，拒绝其它run/ledger/serve误接管；Root关闭后不新开模型收口，正常执行关闭屏障。发现pending grant网络回执与root关闭竞态，正在平台普通revoke以具体generation/tombstone收敛，不能重新mint已关闭root权限，也不能将撤权当物理停止证明。第二轮单任务cargo check8.71s通过，待最后接口落地后构建实际操作binary。

本轮实现收尾见 [autonomous-mode-results.md](autonomous-mode-results.md)：真实无模型平台/CLI/Console操作、旧平台副本升级及浏览器表单核对通过，所有本轮服务退出。源码和正式文档已完成；必要失败保持原件。旧GitHub+Pi剩余资格及新run真实模型资格未转写为通过，保留这一packet供后续资格工作。

最终收尾：单任务离线构建17.17s、JS语法与diff检查通过。旧平台真实回执重试的409兼容缺陷已修复，原请求公开API回读200/applied且无重复对象；成员讨论/设计回流也完成实际操作。最终证据与限定见 autonomous-mode-results.md，所有本轮监听已关闭。下一步提交本轮改动并更新现有Draft PR#148；不将编译及无模型操作扩大为真实自主模型验收。

## 真实 run：ARC-bench lite

2026-10-12 用户授权：“执行真实 minimax 的 braid run 整轮验收，验收题目就复用 arc-bench lite。”本轮两题 keep、bookstack 按顺序从公开需求生成，独立 Braid e7615e9 / binary2c20f706 驱动内建平台 run；全部根/子模型固定 minimax/MiniMax-M3，官方凭据仅从 agentic26/.env 加载，不输出。maxConcurrency2，不设人为费用停止线，不复用旧对象/ledger或手动API操作现场。所有Mac产物位于 /Volumes/WorkSSD/Development/braid-run-arc-lite-20261012，保留版本、输入哈希、原生会话、对象/PR、最终交付commit、OTLP及实际费用证据。

完成判据：实际模型从单一root/description建立自主工作图并以真实PR候选交付；平台范围闭环及执行停止确认，结构化RunResult可追溯。每题交付立即冻结commit与应用包，独立实际验收及在可用入口取得官网self_funded非正式评分；不将 completed、上传或运行启动视为业务通过。生成只读取公开requirements/引用资产，不读取评测实现；隐藏评分不反馈仍生成中的Agent。设施可修复缺陷属于闭环，保持失败原件并修复后继续；不运行设施测试。

负责人：native_local_env继续拥有运行装配、真实执行及恢复；arc_lite_evaluation_owner拥有冻结后独立评分与应用操作；主Agent采用证据、协调修复及报告。当前正在冻结配置、公开输入和评估接口，近期官网403作为待核实外部前提，不能先判零分或宣称评分可用。两题业务产物不得由开发Agent代写。支持记录 run-arc-lite.md、运行目录及evaluation/。

启动事实：Keep公开CLI run-id keep-20261012、root#1、delivery refs/heads/run/keep-20261012、并发2；PID65775、工具session28195由native_local_env拥有停止/恢复。平台18582、OTLP-only Console26518，sample1.0。最终binary身份及原始公开需求SHA见raw/frozen-identities.json，运行记录见run-arc-lite.md。BookStack尚未启动；WSL固定镜像评估已准备，不把CLI启动受理视为模型/交付完成。

真实启动发现设施缺陷：Keep仅握手MiniMax-M3，无prompt/原生JSONL。ConfigureRoot产生invalidate随后Assign，Context reset applied但continuation=false，初始wake已consumed，root静默。保留keep/raw/startup-otlp.json及原运行，由run_runtime_owner支持调度/reset根因修复，native_local_env持续拥有控制和部署；不以人工评论激活遮盖初始输入丢失。此时没有有效生成或业务评分，BookStack未启动。

修复候选861a15d：finite Issue Assign保留真实wake，materialize后先处理已有idle Context reset；serve指派仍仅建立idle会话。单任务离线构建15.09s通过，binary d8a3342f58ed4887b5040ac179997bc95742c8e89265a263172f2e8d644482c6。native_local_env已获新候选部署入口，以新root/run v2验证真实指派触发推理，旧run保持stopped故障证据不可原生接续。

修复实际采用：Keep新run keep-20261012-v2/root2已出现真实prompt→agent_start→MiniMax-M3 assistant/tool执行，keep/raw/v2-startup-evidence.json保留，不以握手代替推理。尚未交付；BookStack未启动。

第二次设施故障：真实M3普通bash经PBB覆盖入口使用bash -lc；host /etc/profile的Nix source在open阻塞，连pwd/ls含60s也超时。sample和原始调用保留，无子Issue/PR/业务产物。公开stop达到15s截止后typed blocked/stopConfirmedfalse，较晚OS空快照未改写原结果。修复9e62be1仅自动PBB shell-c及原生wrapper清BASH_ENV/ENV，明确环境不依赖宿主profile。一次旧冻结公开offlinecleanup于北京时间14:44成功取得exactexecution完整scope收据，keep/raw/later-offline-cleanup.json；原deadline失败保留。新immutable材料准备后使用新root/run重新资格，不称完整恢复。已记v2实际M3 usage totalTokens558899，记录不等于供应商账单。

新原生材料native/pi-nonlogin-v3 manifest989a1d3fb888b1cbb3404d8e597a88147295b5da1626c24ddae0b5c8e17fcc18，source9e62be1；实际SDK公开PBB echo/pwd/ls约200ms正常且execution停止完整。operator一次缺detached被75拒绝原件保留，没有改错误为pass。doctor新材料通过，Keep新run keep-20261012-v3/ref refs/heads/run/keep-20261012-v3已启动，binary861a/d8a保持；旧config/result未覆盖，待真实自然工具与工作图。

Keep v3真实语义进展：普通M3 bash自然调用约200ms成功；root先在自身worktree写应用/做健康及CRUD操作，随后自行创建并指派后代Issue4设计架构、5backend、6frontend、7integration/delivery。scope3为父子范围权威（Item读取不包含parent字段，不将字段缺失误判无关系）；v3-scope-stage6.json保留。此时无PR、delivery仍需求基线b59c，不能把图形成判为成果采用/交付；需真实成员turn、PR候选和最终闭环。

Keep v3整轮程序已exit0：completed/root3/deliveryab8c1d763f7790722938a5e0834ae2523711b6d3/stopConfirmedtrue，原stdout与scope-stage9。实际merge9→e7c8、12→ea5e、13→ab8c，未决命令/merge空、exactowned四收据均stopped/fullcoverage。自主分工不通过：仅root模型，子Issue4–7和PR8–13均由root自行收敛，无childIssue/PR模型session；不能用图和completed代替成员成果。Keep准确commit已archive冻结评分，官网self_funded lite上传实际HTTP403 Only Playground submissions and run changes are allowed，无submission/run/官网分数。WSL固定官方公开32项正在评。

BookStack同配方已启动：run bookstack-20261012/root1/platform18583/CLI工具session25269，新native989a/binaryd8a未热换，无评分反馈。SQLite code5自然调用错误已定位CLI Bind的DEFERRED read→write争用结构，517不是原实测；3d7e712写事务BEGINIMMEDIATE/读DEFERRED+错误细节编译20.35s通过，候选7dd4...未进入本轮模型配方。待最终真实公开connect操作取得有限采用反馈，避免声称稀有竞态已完全验证。Keep成功轮187assistant/167tools、recorded totalTokens12797376含cacheRead12504213（实际provider仅M3），加v2失败消耗558899；SDKcost0不是账单/免费证明。

BookStack自然终态：exit1/blocked/root1/deliveryCommitnull/stopConfirmedtrue，reason root closed without a published or merged scoped candidate matching delivery ref ... d0e8f98ab958d56bd3089ef357f78f9a59898095。所有对象关闭，但known deliveries=[]；root只有一个真实turn，直接写子工作树，三子及PR5均无成员模型执行。冻结d0e8仅作为失败运行生成应用快照，不称accepted delivery；评估owner开始独立34项及官网接入。禁止人工补publish/merge、重开/重复生成。当前所有生成已自然终态，剩余评分、精确资源收尾、当前源码公开绑定操作及最终报告。

本轮最终采用：评分8/66（Keep6/32、BookStack失败快照2/34），官网两次403，无有效官网分数。所有actualmodels仅MiniMax-M3，含失败v2recorded totalTokens30807020，其中cacheRead30126225；SDKcost0不是账单。精确owned收据全stopped，平台/collector正常退出，声明端口无监听；独立WSL两容器已退出移除。已确认归属的5份正文、keep_backup和_server.log迁入WorkSSD并保留原路径/时间/hash；未创建_home，旧/tmp/keep-test非本轮归属未动。pkill工具整体timeout，独立exit及victim未知，不追认其合规。新3d7候选一次真实公开object show/Bind exit0；稀有锁竞争未重现、不声称完整资格。最终不通过，停止新增模型运行，等待用户下一轮决定。
