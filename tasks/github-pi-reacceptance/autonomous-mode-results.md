# Root 自主运行实现与无模型操作

2026-10-12，用户以“是的，开始。”授权实现。`braid run` 已接入既有 Harness，CLI 和 Console 接收 root Issue 或 description；built-in 仍是独立 minimal GitHub。当前交付是源码实现、编译和真实无模型操作，尚未取得模型自主拆分及交付资格。

运行合同已归入 [run 指南](../../docs/user-manual/run.md)、[产品范围](../../docs/10-prd/scope.md)和[生命周期](../../docs/20-product-tdd/lifecycle.md)。平台 schema 3 原子保存父子关系并绑定顶层 root/run/client/ref；Harness schema 7 保存独立运行身份和结果，工作图没有复制到 Harness。实际角色、调度、writer 撤销与物理执行继续复用现有机制。

所有操作产物位于 `/Volumes/WorkSSD/Development/braid-run-operations-20261012`。第一次构建 SHA 为 `e252b4cc4836bd13a7fafa7668c4ab51d9e6713c71c165945db3cb92e6fd2986`；修正常停止显示后为 `a5df9afcac5a4735930eac5d7946f51891990db5357556f539c41fa3b9a04ac7`；含 Run ID 接续表单的 binary SHA 为 `de3fb010cf02647893392a087f9f9c01cc6f1054a2b3c465c4e7f1d145c70f8e`；最终含成员交接通知及旧请求兼容修复的 SHA 为 `2c20f70610a40563333d09d6c1abbc584ea08164f4742e5bf253ef4ef5d35015`。构建均 `cargo build --locked --offline -j1`，最终 17.17 秒无 warning；第一轮 check 的两个接线/SQLite 类型错误已修复，原工具回执保留。JS 通过实际 Node `--check`，没有编写或运行设施测试、自检或模拟 provider。

## 已观察的产品操作

| 结果 | 实际事实与证据 |
| --- | --- |
| 平台范围操作通过 | root 建子 Issue、编辑子项、建真实 PR、实现者发布、root 合并到独立交付 ref；手动资料候选，不冒充模型业务生成。`raw/platform-operations.json`。 |
| 幂等与拒绝通过 | 相同 child request 回读同对象；改变 payload 返回409。写无关对象、跨范围 Link、Agent 改运行授权、不同run/client接管、向 main 建 scoped PR、root 发布他人 PR均被拒绝并保留原错。 |
| 关闭屏障通过 | root 在子项仍开放时关闭，旧 root grant 随后写子项返回409；新 grant被拒绝。pending absent revoke生成撤权tombstone，旧epoch激活被拒；撤权回执不当作物理停止证据。 |
| 输入与恢复通过 | 空首行 description正确创建root#6，正文保持原文；专用 `BRAID_RUN_OPERATION_KEY` 未设置，实际恢复机会后 blocked，stopConfirmed=true。相同run接续仍root#6，没有重复创建。`raw/console-operations.json`、`raw/no-key-root-scope.json`。 |
| CLI结果与输入绑定通过 | closed root CLI保留blocked JSON并非零退出；相同输入回读；同ID改输入拒绝，未覆盖原结果。`raw/cli-operations.json`。 |
| 控制与监督恢复通过 | transport-only + run 返回400；控制请求回读/换payload拒绝；无模型轮次的运行中公开stop返回stopped且stopConfirmed=true。Console正常退出后重开保留run/停止事实，Harness snapshot读取run实际ledger。`raw/stop-control.json`、`raw/normal-stop-fixed.json`。不能称为活动模型停止验收。 |
| Console实际界面通过 | Helium浏览器实际观察run选择、输入和独立终态；发现缺可编辑RunID后补齐。最终“填入最近运行参数”预填stop-control、原description、ref、并发2，不把原description变为生成root#7，不自动提交。两临时tab已关闭。`raw/final-observations.json`。 |
| 既有平台迁移通过 | 从真实schema2原件只读backup到独立副本，再用公开platform serve实际升级3。items2/comments5/links1/events10/merges0/receipts14前后保留，自动备份生成，repository API200；原件SHA及Git状态不变。`schema-upgrade/before.json`、`after.json`。 |

实际成员交接操作确认：同一 profile 的子项写讨论和设计，祖先收到仅含对象与变化类型的 Wake；root 自己写子项不产生回流循环。`raw/child-outcome-events.json`。这些是隔离数据集中的公开 API 开发操作，没有启动原生会话，不将该现场作为后续付费模型的恢复检查点。

旧 schema2 的真实已应用 CreateIssue 请求在初版升级后重试返回409：新增可选字段序列化为 null，改变了回执请求哈希。现省略默认空字段，并同步平台与客户端 grant 序列化，不修改旧回执或添加哈希回退。最终 binary 使用原 requestId 和原 payload 重试返回200/applied，仍是旧 Issue#1，items2/receipts14未增加；原数据库 SHA 不变。`schema-upgrade/legacy-object-before.json`、`legacy-object-after.json`。没有可重构的旧 grant 请求，未将对象回执证明扩大为旧 grant RPC 实跑。

实际操作也纠正了一项静态猜测：旧祖先grant能否穿透注册子root。公开注册返回409 `run root cannot be a child Issue`，该对象并未注册，后续写入属于普通serve子项合同，不能记为run越权失败。保留顶层root及父子不可重绑不变量，撤回推测性新增检查。`raw/ancestor-before-fix.json` 的文件名来自当时假设，原记录未覆盖。

正常stop曾被Console映射为错误；已把RunResult.reason与失败字段分开，重开及实际再stop确认runtime.error=null、health.last_error=null，同时保留reason。浏览器实际发现的接续表单缺口也已修复。两者都保留前版事实和后版证据。

最终runtime stopped/error=null/stopConfirmed=true，当前run ledger sessions0/turns0；Collector122记录、37批次，采集不用于完成判定。没有加载供应商凭据或启动Pi原生推理。Console和临时migration平台均正常Ctrl-C exit0，26418/18482/18492/18481无监听，未控制其它进程。开发侧资料候选合并为 `fee44043b636b30bef54dfd280f0e917565a22bd`，仅用于平台实际操作，不是业务应用验收。

## 尚未取得的资格

真实模型从单一description自主拆分、子任务交接、并行准入、PR实现及completed整轮尚未运行；活动模型停止、真实授权响应丢失竞态、Context reset和原生历史接续未在此模式实跑。原GitHub+Pi验收的失败及未执行项仍独立保留。本轮不以无模型HTTP/CLI操作或编译宣称自主运行验收通过。

maxConcurrency仅限制在途Braid turns，默认2；不保证空闲物理会话、Pi内部subagents/background或宿主总资源有界。当前root必须顶层，首版run仅builtin；跨机器完整崩溃恢复及外部GitHub自主模式没有新增承诺。
