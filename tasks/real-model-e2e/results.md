# MiniMax 双上游真实端到端验收结果

2026-10-11，按用户“确认，允许，开始。”授权，在独立 Braid 执行内建平台与 `xiaoland/braid-poc-test` 的真实模型旅程。两上游均取得真实模型生成、更新、发布与合并的应用，修复后覆盖本轮控制和生命周期范围。首次尝试存在失败，全程存储及隔离合规未通过；本轮不是无纠正黄金路径，也不宣称完成 tagged release 或 Codex campaign 资格。

源码修复提交为 `3fab0d0`。GitHub 最终实际 binary SHA-256 为 `728df182e9461e9150b8dee3c3bd2bbfd86989c9d08b58c457e441c8a6067e62`；内建收口版本为 `81f963d0d5b7b4597e3707527deb0e01c4775f99197e7be242b82e84a556ccd9`。两者是本地源码候选 0.3.2，记录各次部署身份，不以最终提交替代原运行版本。

所有模型均为 MiniMax 官方 `MiniMax-M3`（需求名称 `minimax-m3`）。凭据仅从用户指定文件加载，报告与配置不包含 key。执行、日志、原生会话、原始证据及应用提取位于 `/Volumes/WorkSSD/Development/braid-e2e`；发生过的系统盘写入另记为失败，不因后来回收而追认合规。未启动 Factory、赛事运行或外部应用评测。

## 真实产物与采纳

两上游从仅含需求的空白基线独立生成相同用途的 Python 笔记 CLI。Issue 角色维护设计，PR 角色实际实现和发布；开发 Agent 只驱动公开协作动作并实际调用固定候选应用，没有代写业务答案。

| 上游 | 发布的设计候选 | 实际合并 | 独立应用操作 |
| --- | --- | --- | --- |
| 内建 | `d639ff30d6a079fb5ede8dbd8a884feeaca3a4c0` | `77c8f50a5323756dc85d7a8e7f2dde2d91526c6f` | 10 次操作符合要求 |
| GitHub | `08c2458751a02a8143e0cb7fe620c20bd48dea16` | `8281fffa0bb76398a0cd4c262bd69351e9ed2da6`，[PR #38](https://github.com/xiaoland/braid-poc-test/pull/38) | 10 次操作符合要求 |

固定候选操作覆盖中文、引号、换行正文、同标题唯一 ID、跨进程持久化与创建顺序、归档保留正文、默认排除归档、`list --all` 的状态、重复归档不改数据，以及无效 ID、损坏 JSON 和错误根结构的具体非零错误与原数据保持。两次合并都是真正的两父提交，文件树与已验候选完全一致。首候选与失败候选另有原件，不混成最后采纳内容。

## 本轮判据

`pass` 指有本轮真实操作证据；`fail` 保留已观察失败；`unavailable`、`not-run` 均不算通过。

| 判据 | 内建 | GitHub |
| --- | --- | --- |
| 真实输入、角色分工、模型生成与发布 | pass，Issue 空 bootstrap、正常人工指派 PR | pass，Issue 自主 ensure Draft/关联 PR，真实官方事件自动激活 |
| 关联设计变更和独立应用操作 | pass | pass |
| canonical Context 可见／隐藏标记 | pass，首输入及修复后的公开 context 重读；raw show 曾含隐藏正文 | pass，首输入与新设计／活动更新后的输入；不扩展到任意工具读取 |
| 普通讨论合并、quiet、等待时停止与重启 | pass，原待处理批次恢复后单轮消费 | pass，两条讨论同批次单轮；App 自身评论 origin echo 不自循环，git push 作为外部 synchronize 单列 |
| 空闲与活动正文更新 | pass | pass；首次误报 active 已更正为 idle，另以即时 running 记录验证真正 active reset |
| 关闭／重开／合并 | pass，关闭后只读修复，合并单次收口并退休 | pass，closed 收口 sleeping、reopen 新会话、合并等原执行结束后单次收口并退休 |
| 改派、撤销与迟到写入 | pass，原合法旧 cap 携当前 revision 实际 HTTP 409；最初格式拒绝不计 | unavailable：普通 App trusted mention 模式，不模拟 native assignment 或内建 grant |
| 停止及退休后普通输入／重启 | pass，公开 stopped、真实 Pi wait；退休后 PR turn 总数保持 9 | pass，公开 stopped、真实 Pi wait；退休后 PR turn 总数保持 11 |
| 上游在 Harness 停止后存续／网络补齐 | pass，平台独立存续并保留对象与游标 | pass，官方 webhook 530 后 reconciliation 实际 ingest→batch→turn→原生引用同评论→完成；原失败 delivery 不重投 |
| OTLP、Console 与停止后历史 | pass，11193 条记录及历史 trace 可读；Diff 竞态已修复并真实验证 | pass，15357 条记录及历史 trace 可读；对象权威仍在 GitHub |
| 无纠正首轮黄金路径 | fail | fail |
| 全程存储与空间隔离合规 | fail，实际工具曾写系统 `/tmp`，限定归属回收不抵销原失败 | fail，配置继承缺失后模型触及旧内建空间并直接改旧 ledger 后恢复，不声称未触 |

末次重启未重新创建原生会话。真实 Pi 退出证据属于最后仍有模型的代次，不把末次无原生会话的 stopped 状态当作新退出证明。首次内建 provider 停止 unknown 仍保持 unknown，未解锁或修改其账本。

## 修复及未抵销的失败

本轮真实运行暴露并修复：内建 ready 仍按旧 local 判定；忽略文件被候选清洁检查误算；Pi 保留 stdin 等待依赖 EOF 的进程退出；关闭后只读 context 错依赖 writer grant；merged 事件未进入统一终态／退休；Diff 内容缺少固定对象与提交身份；canonical 重读指引误指 raw show；GitHub 原生会话遗漏当前配置路径。严格 Clippy、构建与差异格式检查通过，实际接续与应用操作提供产品反馈；按用户指令未运行 Braid／Factory 测试。

验收设施错误也保留：App 临时路径遗漏 `/webhook` 导致官方 404；人工 signed ping 的假 node 污染首 GitHub ledger，真实官方事件因此 503；新 attempt 工作目录替换错误导致 blocked。后续使用隔离 source／ledger／native／Collector、真实正常评论与同需求空白基线继续，没有放宽产品身份约束、私改状态解锁或重投旧事件。

首 GitHub 业务候选 `e084853` 的提交／推送早于已观察跨空间操作，独立 9 次应用操作符合原要求；此时序不构成完整隔离证明。旧内建 ledger 的仓库名虽读回原值，但发生过 UPDATE，不能写成未触及。模型评论的“未触仓库外”不作为事实。

另记录一次开发协作错误：network 子 Agent 曾向独立聊天“通用 Harness 迭代工具”发送 8 条进展。用户纠正后已停止，未再发送清理、道歉或补充；当前任务的内部协调不能授权联系独立任务。

## 用量与收尾

原生元数据共 646 条 assistant 记录，均为 `minimax / MiniMax-M3`，记录身份无重复；未展开正文或 CoT。内建两次 input 346323、output 102800、cacheRead 6135240；GitHub attempt2 input 658424、output 83872、cacheRead 11237691，首次网络失败 attempt 没有模型执行。reasoning 已包含在 output，不另计。

按本轮准备时 input/output/cacheRead `2.10/8.40/0.42 CNY/M token` 估算总计 **¥10.974644**。这是冻结价格下的估算，实际账户扣费未知；SDK cost=0 不代表免费。原始分项见 `qualification/all-usage-summary.json` 与各 attempt model-metadata-final.json。

GitHub 最终公开 stop 为 stopped/error=null；独立浏览器确认 Collector 15357 条／1146 批次、接收错误及重复为 0，初始 PR trace 仍可读。内建最终亦 stopped/error=null，并保留平台对象、候选和合并历史。

GitHub 官方恢复后 closed delivery `cc498440-c558-11f1-98bb-49512759d9a4` 返回 HTTP 202。App webhook 已恢复 baseline 的 URL/content_type/insecure_ssl，PATCH 未携带掩码 secret，readback 三字段一致；Wrangler/cloudflared 进程已结束，原失败和恢复证据保留。

运行 Owner 结束最后 Console，exit 0；旧 blocked Console 退出 1，保留原 `runtime stop unconfirmed / Pi child exit timed out`，不追认其 provider 停止。定向进程核对无本任务 Braid/Pi/Wrangler/cloudflared；14318、19318、24318、25318、18180、18181、18282、19282 均无监听，临时平台服务已结束。原件为 `github-attempt2/raw/final-console-process-exit.json`；不通过删除数据库清理，原始失败、unknown、模型与产物身份均保留。

未验证 tagged release 安装、Codex、三次完整 campaign、兼容 rollback、未授权身份矩阵和费用账户账单；本轮不授予这些资格。HTML Context 过滤与精确配置绑定不是对同账户工具进程的安全沙箱。

## 证据入口

- [批准的执行方案](plan.md)、[准备记录](readiness.md)与冻结的 input/config。
- [内建运行记录](builtin-results.md)、[GitHub 运行记录](github-results.md)。
- [独立产物和 Context 观察](qualification-results.md)。
- [Console 独立浏览器记录](console-results.md)、[网络与 App 恢复记录](github-network-results.md)。
- 原始证据与固定应用位于 `/Volumes/WorkSSD/Development/braid-e2e`；报告不复制 provider transcript 或真实凭据。
