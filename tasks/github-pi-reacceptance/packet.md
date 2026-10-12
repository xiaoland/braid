# GitHub + Pi 真实模型重新验收

状态：2026-10-12，root 自主 run 实现、编译及无模型实际操作已完成，真实自主运行资格未执行；原 GitHub+Pi 第二候选应用复验通过，但剩余生命周期资格保留接续，未判全项通过。用户指示：“好的，现在继续重新验收，先仅验收 GitHub + Pi；全部用 minimax 模型。”沿用已知 `xiaoland/braid-poc-test` 和官方 MiniMax `minimax-m3`（wire MiniMax-M3），凭据只从 ../agentic26/.env 加载，不输出或复制。真实官方按量调用获授权，无人为费用停止线，无备用模型。

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
