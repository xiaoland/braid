# Root 任务驱动的有限运行模式（提案）

2026-10-12。用户指出 Factory26 保留了有价值的运行方式：仅输入 root Issue 或 description，由 Agent 自行拆分 Issue、创建 PR 并实现；希望在内建平台上恢复为与 serve 并列的模式。本文记录最初提案及实施收敛；用户随后“是的，开始。”批准实现。当前正式合同归产品/TDD/run指南；没有启动新模型运行。平台仍是与 GitHub 同层的 minimal GitHub；模式属于 Harness。

建议入口暂名 `braid run`。`serve` 持续服务协作事件；`run` 围绕一个明确 root 完成一次有限工作，输出交付与执行收尾事实。首次实现接入 built-in，不为 GitHub 添加尚未证明的自主运行能力；该限制属于适配范围，不将 built-in 等同于 run。Web Console 继续属于整个 Braid，调用共享运行控制服务并展示 run 身份、root、工作范围和结果，不启动 CLI 子进程。

## 输入与工作协议

已经配置仓库、provider、角色和模型后，业务输入可为一个现有 root Issue，或 description 文件。后者经平台幂等创建 root；恢复同一 run 不再次创建。配置和执行身份仍需提供，不能宣称仅凭描述就能获得仓库和凭据。

root Agent 负责需求理解、拆分、依赖、分派、成果采用和整体整合。子 Issue 维护各自设计，PR Agent 实现、验证并发布真实 Git 候选。Harness 不按需求编号拆任务，不机械猜测业务依赖，也不把模型打印完成作为产品验收。root 用专用协调角色，不全局改变 serve 下所有 Issue Agent 的职责。通用平台协议归 Braid，业务规则和模型配方归调用方角色材料；技能仍按文件发现，不内联正文。

平台持久记录父子 Issue 关系和 Issue↔PR 关系。run 只持有 root、纳入的对象和交付 ref 等执行身份，复用平台工作图，不建立第二套任务数据库。依赖理由、当前共享决定和必要交接材料留在对象正文及 Git 中；仅确有机械门控需求时再增加结构化依赖。

子项终态、候选发布和整合变化必须作为有行动价值的事件回到 root，使其消费成果并继续；不用不断提示 root、频繁新建模型轮次或强制公开回复来维持进展。未清楚交接所需材料时，父正文不自动成为子项全量上下文。

## 生命周期和权限

建议区分 completed、blocked、failed、stopped。安静是观察状态，不是 completed：root 未交付但暂时无可执行工作时继续等待；只有明确缺少前提、需要外部决定或无法恢复的具体原因才报告 blocked，不能仅据静默结束。有效工作、事件、写入或物理停止仍在途时不能宣告终态。

completed 至少需要 root 的明确交付声明、范围内义务已交付或明确取消、有确切交付 commit，以及没有未处理的调度/写入/合并意图，最终原生执行和 writer 收尾可确认。此条件仅证明 Agent 声明与交付流程完成，业务是否满足需求仍由实际应用验收判断。root 提前关闭而仍有子任务不算完成，不擅自丢弃进度或把剩余子任务强制关闭；按运行关闭协议停止本轮执行并报告未完义务。停止确认不完整时不得把 stopped 写成 confirmed。

root 的创建、指派、成果采用权限限定于本 run 的后代及关联 PR；子项不能操作仓库其它任务。需要补现有 writer grant 的工作范围，而不能向 Agent 发 human/admin 身份或绕开撤销机制。保留“关闭对象即使写者失效”的保证，root 关闭是最后完成声明，不延长关闭后的写权限；提前关闭且义务未完则 blocked，由显式外部重开后接续。自动采用候选限于明确交付 ref，是否允许合并及目标由运行配置声明；run 不隐含向用户默认分支合并。

实施校正：并发由 Harness 对实际在途 Braid turns 统一准入，root/子 Issue/PR 均计入，默认 2；空闲物理会话及 Pi 内部 subagents/PBB 不在该参数范围。物理总 cap2 会使 root+idle 子Issue阻断PR，原生 idle close又会撤权、停止后代并被现有恢复立即重建，因此本轮不承诺总物理资源上限或新增自动驱逐。不能只在提示里要求少用 Agent，具体并发在运行配方中冻结。模型 fallback、费用方式和停止策略也沿用调用方明确配置。

## 当前实现依据与缺口

- `src/runtime/builtin.rs::serve_managed` 仅 external stop 或 worker 结束退出，没有 root 完成条件；复用其启动、事件、健康和 shutdown 机制，而不是再写一个独立 provider 调度器。
- `src/platform/objects/commands.rs::ObjectCommand` 已有 createIssue/createPR/assign/publish/merge，命令与请求回执可复用。
- `src/platform/schema.sql::local_links` 只有 Issue↔PR，没有父子 Issue。`src/platform/grants.rs::validate` 只覆盖 own item 或直接 Issue↔PR，无法直接给 root 完整子项协调能力。
- `src/group/provider.rs` 当前 Issue 默认讨论设计，需在 run 中显式选择协调职责；不要将自主拆分指令导入 serve 的通用 Issue 角色。
- 现有 Console Harness snapshot 主要展示执行表，不能替代 run 的义务范围与终态合同。平台对象和 Harness 执行事实应分别读取，OTLP 不作为完成权威。

内部 advisor 已定向核对 Factory `sources/braid/src/local.rs:264`：旧实现区分 quiescent、delivery_complete、execution_settled；`variants/pi-braid/run.py:301` 还检查 root CLOSED。可迁回的是有限运行闭环和动态指派协议。variant 的基础 PR、最终整合 PR、Web 应用目录、模型、ARC 发布、专门 reviewer 等是调用方配方，不作为 Braid run 固定阶段。平台创建子项时应原子建立父子关系，不能靠事后任意关联将无关任务纳入 root 权限；旧 local 整体实现不直接覆盖独立 Braid。

验收应从单一 description 实际触发 root，自主产生至少两个可分工子项与真实 PR，展示依赖成果消费、最终整合和确定交付 commit；另实际操作空闲未完成、提前 root close、停止、接续和幂等输入。保留应用验收及所有错误，禁止设施测试或换名自检。当前不运行此实验，也不把先前手动 Issue→PR 验收当作本模式已经通过。
