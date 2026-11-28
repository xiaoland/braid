# GitHub + Pi 本轮执行

2026-10-11，执行负责人 native_local_env。本轮已按用户“你悠着点”的性能约束停止并完成外部接入恢复，没有在途操作；不新增模型或重新启动。第二候选 `95fa210c8e5692be1af8c89fc8e2f128ff1a8911` 经主 Agent 独立16次实际 CLI 操作采用。首次候选的创建顺序错误已由真实模型修正，原错误仍保留。整体纪律判定 FAIL：真实模型曾写 `/tmp/evidence-comment.md`，也曾以 kill/pkill 名称匹配停止服务，后续纠正不抵销。PR [#40](https://github.com/xiaoland/braid-poc-test/pull/40) 保留 Draft、未合并、未关闭重开；Issue [#39](https://github.com/xiaoland/braid-poc-test/issues/39) 与 Git、原生会话及观察库保留供后续低负载接续。整个验收计划未完成。

运行根 `/Volumes/WorkSSD/Development/braid-gh-pi-reacceptance`。正式运行源码为 `89505d964314cd6f65b928543f33c59536fde386`；Rust binary 编译来源 `13943195dfb9b162e99fda6187fd0f238d0afdae`，SHA `f015b77bdc03ee44f42dcec6c62879abd04f72e2b68254d6a37725a977fcac95`。实际材料 native/pi-attempt2 使用官方 Node24.15.0、Pi1.0.2、subagents0.56.0、background1.0.5，manifest SHA `d1fd63d831d2753a21c0b22e59df5bddaee0ca3fbc04cf48485f76edff2bb9bc`。首份 Homebrew Node 的 dyld 失败及两次 fallback 配置解析错误都作为准备失败保留，未覆盖为成功。

公开 idle stop request `c77a14da-e1e1-4bf6-8f38-1845c70ac0ff` 最终 applied、state=stopped、error=null；停止前两当前会话 idle、零 running turn，不能称为公开 active stop 验收。两个当前 execution 的 offline receipt 均 exact identity、status=stopped、scope=execution、coverage=registered-process-groups-and-inherited-execution-markers；实际 Rust Child.wait 记录均 graceful_eof/exited/exit status0。证据为运行根 raw/stop-receipt-performance.json、final-owned-stop-receipts.json、final-public-stop-child-wait-records.json。此前 Issue idle 与 PR active Context reset 已有真实终态、旧 execution 收尾和替换接续证据，独立计通过。

App webhook 原 url/content_type/insecure_ssl 已实际 PATCH200 恢复并 GET200 核对相等，未回写遮罩 secret。Console session91852/PID24988、tunnel session13651 均正常 Ctrl-C exit0；26318/18380/18381 无 LISTEN，已知 PID24988/65289/65611 不存在，raw/final-port-process-checks.json 保留。未操作其它高 CPU 进程。Console 停 Harness 前一次62.7%CPU，停后一次0.0%，随后关闭；这不是新性能修复的部署效果。主 Agent 后续源码提交 `d9f52d9` 删除 idle readiness 每2秒两组重复全量28,640材料文件哈希，启动/恢复/实际 spawn/doctor 仍校验；只完成源码编译，未重编部署或重跑模型。

root/child 实际模型均只有 `minimax/MiniMax-M3`。raw/native-model-usage-summary.json 汇总10份物理 JSONL（3 root、7 child，排除重复 transcript；native session ID+entry ID 去重，重复0）：input515803、output161755、cacheRead14516074、cacheWrite0、totalTokens15193632。reasoning22833 属于 output 的细分，不重复相加。原生 SDK cost 各字段为0，不代表免费，也不能替代供应商实际账单；中断流可能缺少完整 usage。

定向原件还观察到原 PR 会话在14:07:52.835Z与14:07:55.164Z两次自然 HTTP429：`rate limit exceeded(TPM) (1039)`，随后同 turn 恢复生成并完成，没有人为注入失败或操作者重试。既有物理 JSONL 没有显式 retry/willRetry/auto_retry 条目，因此只能判自然限流后恢复已观察，精确 retry-intent 事件合同证据不足，不能概括为自然重试完全通过或完全未运行。14:14:10 的 Request aborted 属于真实 Context reset 中断，分开记录。

已观察真实同步 async:false 与异步派发/等待/消费，有限 PBB exit1 原错误与结果投递、后续 exact background handle stop 均保留；原名称 kill 不计 owned stop。Console traces/logs/metrics 三信号已由主 Agent 实际 UI 读取，最终 collector15972 records、receive_errors0。公开 active stop、close/reopen、merge、restart 及重启后防重复等剩余项目未执行，不补造通过证据。

以下为各阶段当时的操作记录；其中“等待”“尚未”等是历史阶段状态，以以上收尾状态为准。

2026-10-11，执行负责人 native_local_env。当前阶段为真实 Issue 设计生成，已创建 Issue39 并经可信评论启动正式 MiniMax 调用；准备故障保留在下文。运行根为 `/Volumes/WorkSSD/Development/braid-gh-pi-reacceptance`，本轮需求目录 `e2e-notes/20261011-github-pi-native-v2/`。源候选 `13943195dfb9b162e99fda6187fd0f238d0afdae`，binary SHA `f015b77bdc03ee44f42dcec6c62879abd04f72e2b68254d6a37725a977fcac95`。

根角色固定 minimax/MiniMax-M3；仅 design-review 与 implementation-review 两份显式子角色，disableBuiltins，严格 modelScope 唯一允许同模型，不启用 fallback、外部 runner 或其它登录。models.json 只引用官方 API 环境变量，凭据只从用户指定 .env 临时加载，未输出或持久复制。材料只启用 subagents/background。HOME、临时、缓存、数据和所有证据均在本轮 WorkSSD 根下；既有 App 密钥只读使用，不将原件移位。

新的远端 clone 和独立 SQLite 账本已经建立，公开 migrate 0→6。source 使用 sparse checkout 仅展开本轮业务目录，正常 main 历史仍含旧应用，角色明确禁止读取它们，不能声称远端是空树。需求空基线只有 REQUIREMENTS，不含实现答案；正常提交/push 与 HEAD 身份将在首次运行前补记录。没有复用旧 ledger/native/observations。

App webhook 的当前基线与 identity/installations 已通过真实 GET 200 保存到 network，权限0600；尚未临时 PATCH。安装范围仍须在开始正式对象输入前核对，不以共享 App 身份推断远端无人使用。端口计划 Console26318、ingress18380、health18381。

准备操作的 Python 系统 CA 缺失和隔离 HOME 后 certifi 不可见均在操作脚本侧修复，使用 WorkSSD 中公开 CA 原件，未关闭 TLS 校验；一次配置生成语法错误未写入文件，后续修正才产生冻结配置。没有以这些失败启动模型。

实际 doctor 发现首个交付失败：材料 files SHA 校验通过，但复制的 Homebrew Node 缺少 `@rpath/libnode.147.dylib`，Pi真实启动 dyld 失败。原安装目录 native/pi 和 raw/doctor.json 保留。稳定 pi_plugin_feedback owner 正在修复固定 Node 完整交付，后续使用新的 native/pi-attempt2，不能覆盖旧失败。冻结材料 SHA 为 `4a87c7d96b0b0fe59cf53fc78815f23c96dd0aaf9b3d48f730e3d477030fdc9b`，该身份当前不具备可执行验收资格。

新需求空基线 `4c49fff0a2e5c778c57dc627ef3c2ded06f10d75` 已正常 push main，仅新增 REQUIREMENTS。首次 git push 在隔离 HOME 中没有 credential helper 而被拒绝，接线 gh 后实际 push 成功；系统默认 keychain store 报 `failed to store: -60006`，已在新 checkout 本地清空默认 helper 链，仅保留 gh auth git-credential，后续不写宿主 keychain。原错误和成功远端 OID 均保留。Console26318已可读，runtime stopped/generation null，无模型；真实 tunnel 已注册 HTTP2，但 App webhook 仍为基线，未引入 synthetic event。

安装修复 source 为 `89505d964314cd6f65b928543f33c59536fde386`，Rust binary 保持1394319编译来源和f015...字节。新材料 native/pi-attempt2 固定官方 Node v24.15.0，manifest SHA `d1fd63d831d2753a21c0b22e59df5bddaee0ca3fbc04cf48485f76edff2bb9bc`，实际 doctor 全部Pass，但它的version检查不代替后续session实际RPC握手。公开start request `37dfdca1-7ec5-447a-81cb-09ed421c9058`，generation `01a12b2e-9f58-7eb3-8ead-8831089c8657` 已ready/provider connected/reconciliation current，仍无模型输入。真实selected repositories仍为braid和braid-poc-test，本轮共享入口窗口已按授权临时接入；恢复只PATCH url/content_type/insecure_ssl，绝不把masksecret回写。

GitHub专用Issue为 [#39](https://github.com/xiaoland/braid-poc-test/issues/39)，13:38:21Z创建，尚未可信mention。官方opened delivery `06dca52c-c579-11f1-9dfc-2779d67b321d` / `3847702079648874496` 在13:38:23.621Z返回HTTP202；使用真实仓库ID1344679384，没有合成payload或redelivery。创建前openobjects=0，创建后公开snapshot无历史assignment/session/turn激活。等待实际子角色配置映射返回后才发送首次付费激活。

实际SDK管理读取先发现 frontmatter `fallbackModels: []` 被解析为字符串候选 `[]`，随后 `false` 同样是字符串候选。settings agentOverrides只适用builtin，不能据此声称覆盖自定义user角色。两次读取都零模型且owned cleanup stopped，原件 raw/subagent-model-config.json 与 fixed 原件保留。已删自定义角色frontmatter fallback行，使用原生未提供fallback的primary-only候选路径，继续严格allow唯一minimax/MiniMax-M3。正式可信mention仍等待最终实际候选数组。

最终配置操作 raw/subagent-model-config-fixed2.json 已确认 rootModel=minimax/MiniMax-M3，两角色真实 candidates=[minimax/MiniMax-M3]、fallbackModels=null，六个 builtin disabled，loader/runtime errors 空，零模型调用且原配置操作 exit0/cleanup stopped。正常可信激活评论于 2026-10-11T13:46:05Z 创建，comment 6109679664，开始正式模型阶段；其原件 raw/issue-activation.json 保留。

Issue 原生 session 2026-10-11T13-46-23-912Z_01a12b36-f328-759e-aff8-47bf69ad3b2d 于13:46:27Z开始 turn01a12b36-fe1d-76d2-8436-92e374d3d815，真实 assistant provider/model 均 minimax/MiniMax-M3，token usage已记录，descriptor cost0不当免费。首 design-review 派发未传async，实际返回 asyncId，须计异步而非同步；run f6c0cac1-1510-490d-8290-cef138597650 已 complete，真实 attemptedModels 只有M3，child metadata原件 raw/issue-child-model-stage1.json。尚待根消费该结果与发布交接；同步义务后续明确async:false。

首次模型设计在公开 Implementation Request comment6109703753（13:48:47Z）提出list跳过坏行、按timestamp/id排序，并把async+wait称同步；这些与原始需求／实际证据不符。需求方正常评论 6109710304 于2026-10-11T13:49:32Z 澄清损坏JSON/错误根结构在list也须具体非零、不改数据、保持实际创建顺序以及明确async:false；未代写业务代码，原错误保留，黄金路径无纠正项不能判pass。

13:52:40Z Issue澄清turn正常completed/idle，无error；已真实async:false design-review调用，待结果详情取证。现在正常编辑需求方原激活评论，仅补一句以最新澄清为准，核对idle Context reset：预期替换旧physicalsession而不自动启动模型turn，保留既有设计/工作树/完成输入，无私有状态改写。PR当前仍running，不编辑Issue正文，避免提前应用冻结的后续业务变更。

Issue同步调用call01a12b3a545c763982904d84已正常结果：async:false、无asyncId、design-review exit0，actual model/attemptedModels只有M3，raw/synchronous-design-results.json。随后原可信激活评论可见编辑实际触发trusted_mention新turn（13:53:07→13:53:37 completed）但status仍context_resets=[]且physicalsession未换；不能据此判idle Context reset通过。保留raw/status-idle-edit-stage.json与snapshot，后续使用不带mention的实际描述编辑核对，不继续追加常规提示。

PR真实MiniMax生成 notes.py及HTTP演示；初次nohup未service:true，30s转finite bg001且wait_timeout；原错误raw/pr-first-service-operation.json保留。后模型自行另起background:true bg002，再显式service:true bg003（14:00:37）并真实curl应用，service端口63878。有限bg001 manifest14:00:18 exit0，bg00214:00:31 exit1，服务bg003 running；终态与消费待分开确认，不能用shell退出推断脱离后代已停止。实际async:true implementation-review 于14:02:30派发run d8de703b-7195-4e9c-9e17-975026749fcd，正常等待实际应用操作结果。raw/pr-async-application-start.json、pr-background-status-stage1.json保存身份。PR同步design-review亦exit0且唯一M3，raw/pr-synchronous-design-result.json。

真实异步implementation-review d8de703b...已complete，processTerminal observed/runner close exit0，无signal，actual child native metadata所有调用M3，parent subagent_wait正常返回isError=false，结果已消费；raw/pr-async-application-stage2.json和pr-async-child-native-model.json。子角色42工具含19次CLI实际操作，不把实际应用操作叫设施测试。

首次业务候选33b12109b69c8a9b4ec6c15c602c4e6c51c6d388已实际push；只新增本轮4文件，Gitarchive原件在candidates/33b...，主Agent独立应用操作。PBB finite terminal在14:09:21以background_bash_result正常投递，保留真实bg001 exit0/bad相对日志、bg002 exit1/No such file，不能作全部成功。服务bg003 abort是子／根模型按名称kill/pkill产生，不是ownedstop确认。另模型14:08:26实际写/tmp/evidence-comment.md且交付声称未触/tmp，本轮存储约束明确fail；原生call/result与公开原评论保留，该具体临时文件已由模型14:08:43自行删除（原cat成功3181字节、发布及rm操作均保留），不追认无违例，不修改业务或ledger。证据raw/pr-scope-deviations.json、pr-finite-result-messages.json。

独立应用首次验收保存17次真实操作，多数功能通过；合法存储副本仅逆向created_at后list --all逆序，创建顺序fail。已正常PR可信反馈要求模型修正且禁止再次/tmp/名称kill，保留firstcandidate。现在按冻结design-change正常编辑Issue39正文接受同标题/唯一ID/list --all，附原始创建先后与错误保护约束，预期Issue idle Context替换，以及关联PR有active turn时硬失效后真实interrupt/ownedstop/replacement接续。保留工作树，不私改ledger；若没有active则只判对应实际边界，不造active证据。

Context reset闭环实际通过：Issue reset01a12b50-25b2... applied/continuation=false，换14:14:34 session后idle；PR reset01a12b50-61f9... applied/continuation=true，14:13:37修正turn于14:14:10 interrupted，新14:14:59 session/14:15:09 continuation running。两旧execution live children-stop及offline execution-stop均exactID/stopped/对应scope/固定coverage；公开Console exactsession records实际Rust Child.wait日志：Issue managed_launcher PID36439、PR39638均graceful_eof exit status0、reason空，证据context-reset-owned-stop-receipts.json/context-reset-child-wait-records.json。不是手工pkill或DB interrupted推断。

修正接续异步验收e5da61a7...实际complete/runner close exit0，64工具，uniqueM3，raw/pr-replacement-async-stage2.json。第二模型候选95fa210c8e5692be1af8c89fc8e2f128ff1a8911已实际push，仅本轮4文件，Gitarchive固定原件在candidates/95fa...；交主Agent再次独立实际应用操作。新会话通过background_job stop使用exact handle，两次service:true均保留实际tool原件raw/pr-replacement-background-contracts.json；本项不追认旧pkill合格。

用户最新性能约束“你悠着点”，root要求立即降载：不新增模型轮次/子角色/close-reopen/重启。第二候选95fa...已固定，snapshot-before-public-stop已确认2当前physicalsession idle、0 runningturn；按原授权公开runtime stop收尾，保留已生成Git/原生/ledger/collector。剩余close/reopen、restart、防重复控制未执行，不伪判通过。仅按knownPID核对24988职责，不清理foreign进程；公开stop不得低层pkill替代。
