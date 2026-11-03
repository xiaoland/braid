用户于2026-10-11已批准本方案并指示“确认，允许，开始。”下文“待批准/未执行”描述为准备时事实；本轮已结束，最终结论见results.md，各阶段原始证据和失败身份保留。

# 双上游真实模型端到端验收方案

本轮以独立 Braid `1fdef82` 为候选，冻结 executable `/Volumes/WorkSSD/Development/braid-e2e/bin/braid`（0.3.2，SHA-256 `50719bcd2adc3e2a261f7a703bf9b572bb0fb2f7db0584edbcbacec6ca081d6f`；本地候选而非 tagged release），外部空间为 `xiaoland/braid-poc-test`，所有 Issue/PR 角色使用 MiniMax 官方 `minimax-m3`，不使用备用模型。先内建再 GitHub，两个运行使用独立配置、账本、Console 证据库和原生会话；Mac 侧路径统一位于 WorkSSD。准备阶段不运行模型或改动外部仓库。

## 固定工作任务与角色

冻结输入见 `input/issue.md` 与 `input/design-change.md`，工作标识为 `20261011-minimax-peer-upstreams`。建议使用不依赖外部服务的本地笔记 CLI，两个上游以相同需求和相同空白应用基线独立实现。基线只含需求、运行说明占位和 Git 忽略规则，不含实现答案。验收程序只实际调用生成应用，开发 Agent 不代写业务实现，也不运行 Braid 测试。

需求正文：实现 `python3 notes.py --store <路径> add <标题> <正文>`、`list`、`archive <ID>`，使用 JSON 文件持久化。add 返回稳定 ID；list 默认只列未归档笔记并按创建顺序输出；archive 后原内容保留且重复 archive 不产生第二份记录；无效 ID 和损坏数据返回具体错误，不覆盖原数据。标题和正文允许中文与引号。无网络、数据库服务和第三方依赖。

Issue Profile 负责澄清需求、维护接受的设计与简洁实施请求，不实现业务文件。PR Profile 只在专属工作树依据当前 Issue/PR 设计实施、实际运行应用、提交真实 diff 并发布简洁证据。两者都用 minimax-m3，角色指令不同；不得把技能正文内联进 Profile。运行开始前记录 binary SHA、Pi 版本、source 提交、两个 Profile 及模型配置摘要，费用值未知时标为未知，不能用 0 当免费。现有 MiniMax 价格记录以人民币/M token 表示；原生 SDK 未配置的 cost=0 不能当作实际账单，报告应分别记录 token 用量、计费来源、人民币估算和账户扣费缺口。

黄金路径完成后，加入明确设计变更：同标题新增笔记必须允许，ID 仍唯一；增加 `list --all` 显示归档状态。通过关联 Issue 正文修改让 PR Agent 接收更新，而非将新要求直接塞进执行器。分别实际新增中文/引号笔记、进程退出再读取、归档、重复归档、无效 ID、损坏数据与重复标题，记录输入/退出码/输出以及 Git diff。原生终态与产物符合需求分别判断。

## 正常操作者动作与上游差异

操作者可以创建/编辑需求、发表讨论、指派或可信 mention、发出停止请求、阅读候选和明确采纳；不代写业务实现、不修私有状态、不注入事件。途中设施修复保留失败和版本，并与无纠正黄金路径结果分开。

内建平台先独立启动，再接入 Harness。Issue Agent 自行发表实施请求、建立仅空提交的候选分支并通过 `braid object pr --head ... --base ... --issue ...` 创建关联 PR。平台要求不同 head/base，不接受开发 Agent 预制答案。操作者通过 Console 指派 PR Profile，这是当前明确的正常步骤；不宣称内建已经拥有 GitHub `gh pr ensure` 的原子建分支/派生/激活能力。PR Agent 实现和提交后自行 publish 固定 OID；操作者阅读真实 Diff 后显式 merge。source 和其他工作树须切离目标 base。

GitHub 使用已接入的 Braid App，先确认 native assignment 或 maintainer trusted mention 的实际模式。Issue Agent 经 `braid gh pr ensure` 交接关联 Draft PR；PR Agent 在专属工作树实施、push、更新 PR 工作记忆。普通 App 不能冒称 native unassign/reassign 能力。外部操作范围限专用验收 Issue/评论/PR/分支及明确采纳，不改其它对象、仓库设置或历史；不自动删除证据。

## 递进验收与完成条件

每项结果使用 pass/fail/unavailable/not-run；后两者不算通过。黄金路径是进入后续验收的门槛，不是整个端到端验收的替代品。

| 阶段 | 两上游共同事实 | 上游特定事实与证据 |
| --- | --- | --- |
| 模型与黄金路径 | 真实输入到达原生会话；设计与实现角色区分；生成业务 diff，应用实际操作符合需求；Agent 自己维护工作记忆和交付证据 | GitHub App/可信 mention、Draft/关联与 ensure；内建请求、人工指派、固定候选 OID及合并 |
| 调度与上下文 | 普通讨论遵守 quiet window；多输入合并，不并行误开；可见正文进入当前 Context，HTML 隐藏正文不进入；关联设计更新后模型使用新设计；自身发布不形成唤醒循环 | quiet=30秒、threshold=8、reconcile=60秒；记录输入、会话身份、触发和时间，GitHub 权限/mention 差异如实标明 |
| 生命周期 | 空闲和活动状态的正文变更、关闭/重开/合并；执行停止与对象关闭分别确认；终态不重复开执行 | 内建改派/同 actor 改派撤销旧写 grant，迟到写入拒绝；关闭后 finalization 不恢复正文写权限；GitHub 使用实际支持的激活模式 |
| 恢复与控制 | 等待期间正常重启仍消费未完成输入；活动执行停止有真实原生确认；未知终态保持未知，不据请求受理宣称成功 | GitHub 隧道中断与 reconciliation；内建平台在 Harness 停止后继续存续、游标接续不重复；活动重启恢复能力按实际证据判定，不假设完整检查点 |
| OTLP 与 Console | sample_ratio=1.0；实际 provider/session/turn、对象输入/写入、候选和停止事实可关联；完整原始错误与用量保留；停止后 Collector 历史可读 | 三类数据库权威分离；GitHub 对象结论取自 GitHub；采集不能替代原生终态或产物验收，费用按实际账单来源估计并标明缺口 |

先在内建完成上述流程、整理缺陷及复跑身份，再在 GitHub 执行同一冻结任务。若某一步失败，保留对象、原生会话、Git 提交、HTTP 状态与 Console 原始批次；修复设施后记录新版本及受影响重跑，不把修复后结果覆盖首次失败。模型无响应、错误或未知终态先有界核对，不无限重试、不自动切模型；需要控制时使用 Braid 公开能力并保留停止确认。

完成本轮需要两上游必需项全部有明确结果、真实产物验收及原生执行停止证据，剩余未验证边界明确列出。现有 acceptance.md 中 Codex app-server、带标签发行安装、三次完整 campaign、兼容 rollback 等发行资格仍有自己的要求；本轮 Pi/源码候选运行不宣称它们通过。若要发行资格，另冻结发行制品与 campaign，不能换名省略。

## 执行前待冻结

核对现有 GitHub App/安装权限、webhook secret、maintainer 与非 maintainer 身份、公网 webhook/Quick Tunnel 接线，以及已有验收对象避免误消费历史任务。独立 runtime 账本不保证不会发现同仓库旧工作项，必须选择明确隔离的 Profile/对象与激活范围。

MiniMax 官方凭据来源由用户指定为 `/Volumes/WorkSSD/Development/agentic26/.env`，只加载 MiniMax 所需变量，不输出或持久复制 key。已只读确认 key 非空、官方 base URL 为 `https://api.minimax.cn/v1`，仍须核对原生 Pi provider/model ID 与 wire `MiniMax-M3` 的对应；不得仅因 catalog 有记录就声称接通。已知 Factory catalog 声明 `minimax-official-m3` 是 pay-as-you-go，引用 `MINIMAX_BASE_URL/MINIMAX_API_KEY`，不等于本轮实际额度或计费已确认。不启动 Factory Lab，也不把 Factory 的执行/存储接线当作 Braid 的依赖。

待真实只读调查收齐后冻结每种上游的配置、端口、运行目录、实际费用模式与启动命令，向用户提供具体执行范围作为最终启动复核对象。当前不请求重复确认仓库或模型。

### Console 与公网入口的部署安排

Console managed GitHub 已拥有真实 ingress 与 Harness 启停，但不托管 Quick Tunnel。
计划使用独立、WorkSSD 安装的 Wrangler 为 `127.0.0.1:18180` 提供公网入口；Console
托管 GitHub Harness 并接收 OTLP，隧道作为外部网络设施。执行前保存 App-wide
webhook 原始配置、核实安装与活动运行的影响范围，经用户认可的写入范围临时配置
该入口，运行结束恢复原配置并核对回执。公网请求实际进入带签名校验的 Braid
之后才能声明 GitHub 事件通路就绪；隧道进程存活不能代替该事实。

如果 App 影响范围或外部入口不能成立，可用原生 `serve --tunnel` 加纯 Collector
验证一部分真实模型流程，但 GitHub Console 控制项仍为未验收，不据此关闭完整
端到端任务。不为了验收凭空新增隧道服务或修改 Console 产品边界。

### 待批准的实际执行范围

开始后允许初始化本轮新的平台/Harness 数据库、启动 Console、原生 Pi 与
MiniMax 官方按量请求，并对隔离任务产生 Issue、评论、PR、空引导提交、实现
分支/push 与合并事实。GitHub 对象均带本轮唯一标识，业务文件限制在
`e2e-notes/<本轮ID>/`；已有工作项与分支不作输入，不修改主 Braid 源码或其它
业务应用。共同 checkout 初始提交为 `6425f01e20ed4894b38ef4adc5da359896d07c20`。
新的需求基线提交仅含说明与忽略规则，不包含答案；仅在执行批准后发布。

MiniMax 模型费用记录实际 token 与服务错误，不另设人为费用停止线，不切换
其它模型。具体使用额度由现有官方凭据决定，不使用赛事或 ARK 通道。缺陷修复
需要区分本轮设施与产品源码：用户当前仅要求准备，尚未授权以本计划直接启动
模型、修改公网 webhook 或执行产品源码修复。执行复核应同时确认本计划的实验
输入、费用模式、对象写入和 webhook临时变更/恢复范围；不以 elapsed time 作为
批准。

### 启动顺序（待执行批准，当前未运行）

固定 binary 为 `/Volumes/WorkSSD/Development/braid-e2e/bin/braid`。两种上游的 Harness
均先经公开 `migrate apply --config <对应配置>` 初始化新 schema 6 ledger；不对旧
实例执行迁移。只有所需模型 env 注入后，才启动带对应 config 的 Console，并
通过页面请求真实模型运行；`accepted` 与 `running` 不代替 ready 或原生输入接收。

内建 Console：`braid console --platform-config <config/builtin-platform.toml> --config
<config/builtin.toml> --database /Volumes/WorkSSD/Development/braid-e2e/builtin/observations.sqlite3
--listen 127.0.0.1:14318`。先以平台为权威建立本轮需求，再指派 Issue Profile。

完成内建记录并确认真实执行停止后，关闭该 Console。GitHub Console 使用
`--config <config/github.toml> --database
/Volumes/WorkSSD/Development/braid-e2e/github/observations.sqlite3 --listen
127.0.0.1:14318`，不装配内建平台。公网网络设施转发独立 ingress 18180，原始
webhook 基线与恢复结果归 GitHub 运行证据；不得同时另开相同 ledger 的
`braid serve`。

实际命令中的 MiniMax 环境变量通过同目录 `load-minimax-env.sh` 加载器传给进程。
模型与 secret 值不写入 shell 历史、配置或报告。异常时先保存具体原始错误，
停止与恢复均按能力和实际确认结果记录。

### 准备核对结论与最后外部边界

2026-10-11 已完成：固定 candidate 0.3.2；两套 schema 3 配置通过公开检查；Pi
1.0.2 原生模型 ID 已统一 `MiniMax-M3`，两个独立 home 只引用官方 key；MiniMax
凭据来自用户指定 env；两个 checkout 同初始提交；Wrangler 4.149.0 在 WorkSSD
隔离安装且 `tunnel quick-start` 帮助契约已核实。当前没有模型请求、平台对象、
新执行账本、Collector、隧道或外部写入。

App JWT 只读核实：安装 `157947940` 的 selected repositories 实际是
`xiaoland/braid` 与 `xiaoland/braid-poc-test`。App webhook 已配置，现有 secret
存在。目标验收仓库当前 open Issue/PR 均为 0，历史对象不得接续。本机没有活动
Braid/Pi/Wrangler/cloudflared；不能据此推断远端无人使用。临时替换共享 App webhook
将影响两个仓库的事件投递，必须确认无其它依赖该入口的运行，并把临时变更和
恢复纳入实际执行许可。若不允许共享入口窗口，需保留现 URL 并准备专用 App 或
经认可的独立接线，不能悄悄绕过。

Wrangler 外层必须设置 WorkSSD 的 TMPDIR、XDG_CONFIG_HOME、XDG_CACHE_HOME 和
WRANGLER_LOG_PATH；有预置 cloudflared 时再设置 CLOUDFLARED_PATH。安装目录为
`/Volumes/WorkSSD/Development/braid-e2e/wrangler`，具体变量见 readiness.md；不依赖
无效的 WRANGLER_HOME。真实运行前保证对应目录存在并核对实际解析路径，依赖
下载、运行日志和缓存不得回落到系统盘。公网可达性只有实际启动后才能判断。
