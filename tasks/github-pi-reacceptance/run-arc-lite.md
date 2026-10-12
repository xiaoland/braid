# 独立 Braid run：ARC-Bench lite

当前：两题已自然生成至程序终态并完成轻量收尾，没有在途模型或本轮服务。Keep v3为completed/stopConfirmed=true，确切交付ab8c1d763f7790722938a5e0834ae2523711b6d3；BookStack为blocked/stopConfirmed=true，deliveryCommit=null，具体原因是root关闭却没有与交付ref匹配的已发布或平台合并候选。其Git ref值d0e8f98ab958d56bd3089ef357f78f9a59898095只作为失败run的实际生成应用快照评分，不当accepted delivery。两题都没有child/PR成员真实推理，所有实现由根完成，根自行创建/关闭子项；自主分工不通过，不能由工作图或typedcompleted抵销。

独立WSL实际评估为Keep6/32（18.75%）、BookStack失败快照2/34（5.88%），合计8/66（12.12%）。全部收集并运行，零skip/flaky/collecterror/OOM，安装启动成功，原件在运行根evaluation/summary.json与resource-exit.json；Keep主要违反可访问角色、名称和状态合同。BookStack32项在入口选择器受阻：Shelves按钮11、Books按钮15、Login form内Login按钮6，不能据此断言未进入的深层功能都没有实现。两份完整无模型self_funded replay官网上传均HTTP403，没有submission/run或官网分数。两个WSL评估容器已退出移除，反馈在生成全部终态后采用，未回流模型。

实际生成使用binary源码861a15d/SHA d8a3342f58ed4887b5040ac179997bc95742c8e89265a263172f2e8d644482c6，以及native源码9e62be14e3724bdb41c22d08c420e1ce26c81506/manifest989a1d3fb888b1cbb3404d8e597a88147295b5da1626c24ddae0b5c8e17fcc18。晚到SQLite源码修复3d7e712/候选SHA7dd4bdba8a3e6587c6aedea5ab989941b9cfcfdcf813df1c9dbcb7803d52c37e没有覆盖生成freeze；结束后用候选公开object show1连接BookStack真实冻结run config/ledger，实际exit0、rootclosed、stderr空，new-bind-candidate-object-show.json保留。只证明该实际读取/绑定，不冒充稀有并发锁竞争全面资格。

两题实际原生根cohort与失败Keep v2都仅minimax/MiniMax-M3，无其它模型、Pi子Agent或子Issue/PR成员推理（实际tool names只有read/write/edit/bash/background_job）。去重recorded usage总计input515140、output165655、cacheRead30126225、cacheWrite0、totalTokens30807020；reasoning940包含在output，不重复相加。SDKcost字段全0不能代表免费或供应商账单，中断流可能不完整。失败v2消耗也纳入，原件raw/total-recorded-model-usage.json及每题raw/final-model-usage.json。

纪律单列FAIL：Keep成功在/tmp写5份Issue/评论正文，并复制app缓存/data到/tmp/keep_backup；BookStack成功重定向/tmp/_server.log，尝试/tmp/_home.html因timeout命令缺失/连接失败没有留下文件。BookStack还执行pkill -f按命令名称停止请求，整体工具最终15秒timeout；无法从该结果确定pkill独立exitstatus或杀了谁，不计exactowned cleanup。具体call/toolResult在每题raw/model-scope-violation-call-results.json。较晚正常owned停止不抵销原违例。

只处置已明确本轮归属材料：5份正文当前字节与原生成功write内容完全一致；keep_backup的1230条目ctime均在原cp操作07:10:52.468–52.785窗口且仅app子树；_server.logmtime匹配重定向时刻且错误堆栈指向唯一BookStack本轮工作树。上述原件已移到WorkSSD raw/recovered-model-system-temp，原真实路径、时间、SHA和备份逐文件清单在raw/model-temp-material-disposition.json及keep-backup-original-inventory.json。/tmp/_home.html不存在；/tmp/keep-test在本轮之前即存在（2026-09-24 mtime），不因模型尝试add/rmdir就认归属，保持未触碰。没有广扫/tmp或杀未知进程，也没有改业务/补发布合并/复跑模型。

每题2个实际execution的root-descendants与wholeexecution停止收据各四份全stopped/exactID/registered-process-groups-and-inherited-execution-markers，见每题raw/final-owned-receipts.json；旧失败v2stopConfirmed=false永久保留，较晚精确offlinecleanup仅说明新时点已停止。Keep/BookStack CLI最终exit0/1分别符合completed/blocked；Keep平台98517、BookStack平台62794、OTLP-only collector44293均正常Ctrl-C exit0。18582/18583/26518/18580/18581/3001/3457最终均无LISTEN，raw/final-port-checks.json。Collector保留16164 records/2175 batches/receive_errors0，观测不当作完成权威。

下文时间戳按原始证据的UTC保留。以下按阶段保留原始身份、错误、停止与采用记录；旧准备和失败状态不替代以上当前状态。

2026-10-12，native_local_env 稳定负责准备、真实生成、范围内设施诊断修复和收尾；主 Agent 更新 packet，独立评估负责人冻结与评分。用户授权真实 MiniMax 整轮验收，Keep 后 BookStack 顺序生成，每题 maxConcurrency2。运行根 /Volumes/WorkSSD/Development/braid-run-arc-lite-20261012，装配要求 Mac HOME/cache/temp/data/原生材料及证据全部位于 WorkSSD；实际模型越界与后续处置见上文。无旧run接管，无开发者业务代写，不读隐藏评测，不编写或运行设施测试。

冻结源码 e7615e9，binary SHA2c20f70610a40563333d09d6c1abbc584ea08164f4742e5bf253ef4ef5d35015，复制到 run 根 bin/braid。新 native/pi 从已冻结Pi1.0.2/Node24.15.0材料复制，manifest d1fd63d831d2753a21c0b22e59df5bddaee0ca3fbc04cf48485f76edff2bb9bc；没有继承 Pi 会话。仅 subagents/background 已锁插件，自定义只读 review 角色显式M3无fallback，builtin disabled与strict allow唯一minimax/MiniMax-M3。根/成员Profile也是官方minimax/MiniMax-M3，凭据只在进程内从../agentic26/.env加载，不复制或输出，禁止其它路由。Pi物理启动还须真实RPC身份核对；doctor不代替模型实际元数据。

公开需求来自 Factory third_party/arc-bench/arc-bench-lite/{keep,bookstack}/requirements，只复制公开requirements正文、参考图片和允许assets。source只有requirements空业务基线，Keep b59c71929290277f23579fcf8b9558b5924c27a4，BookStack67d2b06b85c2c8058f7f178c333d436ab80bff1d；逐文件SHA原件raw/frozen-identities.json。没有读取tests或旧答案。description只提出完整业务与公开协作/交付约束，未替root决定REQ拆解与架构。真实root需自主子Issue/PR和成果消费；应用通过不替代这项协作资格。

应用统一app/根，package.json/锁/README随交付commit导出，npm install && npm run start -- --port 3000 --host 0.0.0.0。每题completed后立即向主Agent与评估owner交付typed RunResult、确定commit、source/app及实际启动入口，不等第二题。独立WSL评估冻结lite Keep32/BookStack34，1worker/10秒/0retry，反馈不注入仍生成Agent。

准备首次doctor在建runtime目录/采集服务前明确fail（目录缺失、OTLP不可达），原件keep/raw/doctor.json保留；公开migrate0→7和真实collector启动后doctor-ready.json成功。非模型测试，不将前置失败覆盖为pass。

Keep已启动：run-id keep-20261012，delivery refs/heads/run/keep-20261012，description keep/description.md，maxConcurrency2。公开CLI进程PID65775（exec session28195），平台PID65247/session95855，endpoint18582；OTLP-only Console PID65276/session93790/26518，没有浏览器常驻与managed runtime重复控制。公开CLI正常终止信号请求停止，须读取程序结果stopConfirmed和实际child/owned receipt；不得以数据库状态或信号发送成功推断停止。stdout/stderr分别keep/raw/run.stdout与run.stderr，平台和collector有原始log。BookStack尚未启动，须Keep真实收尾后顺序执行。

首次真实启动暴露设施缺陷，未开始推理：platform root1 noop→ConfigureRunRoot invalidate→Assign default，所有事件和Assign wake batch变为consumed。Pi真实get_state为M3/Quiescent，Context reset applied continuation=false，旧会话replaced新会话idle，但没有prompt/native rollout。小范围startup OTLP与只读ledger事实用于诊断，无私有状态修改。稳定调度owner接续源码修复，不通过人造评论唤醒遮错误。

为了降载，执行负责人在该idle现场向CLI发正常Ctrl-C。程序实际exit1（stopped按合同非零），typed RunResult state=stopped/rootIssue1/deliveryCommit=null/reason=external stop requested/stopConfirmed=true；keep/raw/run.stdout与stderr保留。未启动BookStack。当前只保留独立平台与OTLP以待必要诊断，后续修复后按已授权配方接续，原启动失败不覆盖。

调度owner确认根因：serve原合同下Issue Assign除mention外会consume batch并只创建idle会话；finite run误继承该策略，root与子Issue都无法执行。ConfigureRunRoot.invalidate仅导致正常idle reset，并非模型失联。修复限定finite真实Assign保留urgent wake，再由既有reset优先顺序先更新Context后实际prompt。旧run已消费且zero turn/pending，不能以sameID重试当恢复；保留keep-20261012的stopped与原配置/ledger，修复binary后使用全新keep-20261012-v2/root与独立deliveryref，不复制业务、不伪造comment唤醒。

中央修复源码861a15d，binarySHA d8a3342f58ed4887b5040ac179997bc95742c8e89265a263172f2e8d644482c6，旧binary副本保留bin/braid-before-finite-assign-fix；旧平台/collector正常exit0后以新binary重开。真实新run keep-20261012-v2 与独立refs/heads/run/keep-20261012-v2启动，description原件不变，新ledger不接管旧run。平台session98517、OTLP-only Console44293、CLI6169；不造评论/子项，也不重复编译。

Keep v2 root2真实推理启动已确认：OTLP prompt record126、agent_start129、assistant record137/160/172均provider=minimax/model=MiniMax-M3，tool execution154/166/178及toolResult真实出现。新reset后原Assign仍可执行；keep/raw/v2-startup-evidence.json保存定向事件摘要，不用握手冒充推理。description无旧ref硬编码，仅runtime注入v2授权。后续按公开工作图/结果判自主协作，不把tokens增长当进展。

Keep v2实际推理遇到第二项设施故障：普通PBB bash实际spawn payload后，`bash -lc` 的ls/echo hi/pwd持续3/5/60秒超时；read目录EISDIR是另项模型工具误用，未混作设施根因。已登记payload可见，定向sample PID76122在run_startup_files→source_builtin→source_file→再次source_file→open/__open阻塞，802/802 samples；宿主/etc/profile含Nix source，定向读取该Nix文件也挂住10秒并由执行负责人正常Ctrl-C结束诊断。sample79937因进程已终止而失败原件保留。BASH_ENV与ENV为空、SHELL=/bin/zsh、HOME为run WorkSSD/home。没有修改宿主profile或扩展超时遮故障；稳定executionOwner收拢受控非login shell修复。原件keep/raw/bash-timeout-owned-records.json、bash-payload-76122.sample.txt。

向当前CLI6169发送一次公开Ctrl-C后程序实际exit1，typed RunResult state=blocked/rootIssue2/deliveryCommit=null/stopConfirmed=false；具体reason为external stop requested及finite runtime shutdown exceeded its deadline; physical execution stop is unconfirmed，保存run-v2.stdout/stderr。不能以CLI退出当停止确认。后一次knownPID76007/76010/76116/76122/76635/76641只读核对均不存在；仍保留旧stop未知。v2-stop-native-receipts.json保存已有exactexecution01a12ecd-8476-7142-9d3c-ff8b3a80c251原件摘要，需Owner收拢当前cleanup事实；不私改run状态、不重复低层信号或开新模型。root尚未产生子Issue/业务app，本轮真实M3推理不能算有效交付或零费用设施失败。BookStack仍未启动。

失败v2的实际模型消耗单独保留keep/raw/v2-recorded-model-usage.json：一个物理会话、38 assistant/37 tools，实际仅minimax/MiniMax-M3；recorded input27862/output2529/cacheRead528508/cacheWrite0/totalTokens558899，reasoning347为output细分，供应商账单仍独立。最后Request aborted在06:37:25.288Z对应公开stop，不当自然模型错误。

原生owner核对只有PBB工具override使用login -lc，Pi核心-c与subagents没有login默认。共享源码修复PBB所有foreground/auto/background/service为-c，wrapper清BASH_ENV/ENV，禁止修改旧冻结材料。主Agent与advisor采用一次旧冻结managed模块公开offline cleanup exactexecution当前时点收据、停止准入fence及registeredPGID/inheritedmarker coverage，然后全新root/run/ledger；原blocked stopConfirmed=false永久保留，后续不称resume，不低层按名kill。精确cleanup和新immutable材料+实际SDK shell操作由executionOwner负责，运行owner不重复控制，等待集中采用回执。binary861a15d尚不需要重建。

新immutable材料pi-nonlogin-v3已实际采用准备：source9e62be14e3724bdb41c22d08c420e1ce26c81506，manifest989a1d3fb888b1cbb3404d8e597a88147295b5da1626c24ddae0b5c8e17fcc18，subagents/background组合不变，28640files actualmismatch0，旧native材料完整身份保持。Owner实际SDK公开bash override echo hi215ms、pwd195ms、ls requirements216ms均exit0；根后代和整个SDKexecution实际stopped，无模型/业务写入。首次SDK caller未detached触发startup rejected75原件保留，修caller后真实操作采用。旧v2 exactexecution较晚offlinecleanup在06:44真实stopped/scope execution，keep/raw/later-offline-cleanup.json保留；旧RunResult stopConfirmed=false不改。两题配置指向新材料，不部署到旧冻结run。

第三次Keep尝试keep-20261012-v3/root3/ref refs/heads/run/keep-20261012-v3，公开CLI session91211启动，新ledger无旧会话接续。自然M3工具pwd&&ls在06:50:58.661→58.882成功约221ms，ls requirements/.braid约203ms；原件v3-first-natural-bash.json。ownedtool登记存在但record不保存argv，短命shell未捕获liveargv，不能编造实际观察-c；冻结源码与真实自然返回支持修复采用。

root已实际成功读取braid object创建/指派/PR/关闭/合并帮助，截至06:55:45尚没有child/PR，却开始在自己的worktree写app/package.json和server/db.js。主Agent决定继续观察自然完整图与结果，基础文件不机械等同失败，不能用root整任务直写替代PR分工；若最终无child或真实PR成果采用，即使应用得分高也单列自主协作不通过。不人工造图、不开发者业务代写、不注入评估反馈、不因几份文件擅自停止或自动换模型重试。

07:08之后真实root自行建立协作图，scope3包含后代Issue4设计架构、5后端、6前端、7整合交付，均指派default，未人工建图/追加comment；v3-scope-stage6.json。公开Item list不带parent字段，因此早先摘要parent_issue:null仅字段缺失，不能当无父子关系；scope/3是范围权威。此时仍无PR、delivery为需求基线b59c...，只判自主图已形成，真实成员执行、成果消费与最后整合待实际观察。root前期自己写应用事实独立保留。

Keep v3程序实际exit0，typed RunResult completed/root3/deliveryCommit ab8c1d763f7790722938a5e0834ae2523711b6d3/reason=null/stopConfirmed=true，run-v3.stdout。真实scope_stage9全部义务关闭且交付非基线，主Agent可立即冻结source ref/run/keep-20261012-v3的app独立评分。package依赖express4与better-sqlite3^11.5，Node声明>=18。

自主分工资格不通过：仅root3一个原生物理会话，无childIssue或PR成员真实prompt/推理；4–7子项均由root关闭，8–13PR均无profile/published=false，root自己创建并采用9/12/13候选整合。公开图与实际交付闭环存在，不冒充成员执行/成果消费。Keep根自产应用仍是实际M3生成，独立业务评分与自主协作分开。PRmerge/base409原错误与模型纠正保留；SQLite code5已交稳定源码Owner调查，不因单次泛化改freeze或丢原错。

Keep两个实际execution各root-descendants/wholeexecution停止收据四份全stopped/exactID/fixedcoverage，v3-final-owned-receipts.json。端口3001无LISTEN，Keep平台98517正常Ctrl-C exit0，不删除对象。BookStack在同binary861a15d/d8a与native9e62/989a配方顺序开始：run bookstack-20261012，root1，refs/heads/run/bookstack-20261012，CLI25269/platform62794 endpoint18583，doctor-ready pass；OTLP44293保留历史库。输入/账本/source独立且只公开requirements。不能把晚于启动的SQLite源码修复部署到此freeze，公平保持当前recipe，具体原code5与Keep自然最终关闭单列。
