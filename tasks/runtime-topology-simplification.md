# Runtime Topology Simplification

## 当前状态与授权

任务进行中。2026-09-18，Human 已确认下述修订设计：Group 拥有逻辑会话与 turn 生命周期，adapter 拥有物理进程、连接、会话寻址与故障范围。原先的「全局共享 provider 连接与 epoch」方案已撤回，不能继续作为实现依据，也不能通过给 Pi 添加例外来保留它。

Human 已授权实现，并要求完整 Slice 3 campaign 验收；smoke、编译或单元测试不能替代。讨论期间也必须及时维护本 packet；Agent 可自主编辑任务记录，无需再次请求批准。2026-09-18，Human 同意重新审查结论（包括 mention 分类归 producer），授权先整理提交，再按更新计划继续实现。

分支为 `refactor/runtime-topology`，基线为 `e27cf7f`。`98e0aa2` 是初版计划提交，其共享连接决策已被本文取代。当前源码均为该提交之后的未提交中间改动，尚未完成验收。后续提交应只包含本任务改动；push、发布 PR 和 release 不在本次授权中。

## 目标与范围

按状态责任组织异步运行时，使代码表达产品的实际流程：

```text
GitHub 状态变化 → 持久化工作状态 → turn 决策
  → worktree 与会话物化 → Agent turn → GitHub 写入收敛
```

本次保留事件分类、debounce 策略、持久化 fencing、outbox 和 Context 状态机的产品契约。不拆分 `store/mod.rs` 或 `context.rs`，不变更 migration，不将 `advance_scheduler` 改为 claim-time predicate。会话创建、恢复、失效通知以及资源所有权接口可以调整，以落实已确认的边界；不能把这一部分描述为纯机械搬迁。

## 已核实的问题

以下描述以基线源码为准，中间工作区已开始调整部分入口。

`producer::ingress::event_worker` 每 250ms 串行执行三类工作：`store.advance_scheduler()`、需要 GitHub 网络请求的 trusted-mention 权限确认，以及 `drain_one_write()`。前两者属于调度决策，最后一项属于写入收敛；权限查询延迟还会阻塞 outbox。

`issue_agent_worker` 和 `pr_agent_worker` 重复实现启动、连接、恢复、turn 事件消费、Context reset 与调度驱动。真正的差异是 Profile 选择、system prompt、恢复兼容性检查与 assignment 物化。已有实现出现健康状态更新差异：Issue 恢复成功后更新 `health.provider`，PR 没有相同路径。

`dispatch.rs` 大量函数反复传递 store、GitHub、config、provider、sessions 和 Profile，说明缺少承载 Group 相关依赖的上下文对象。

原设计对 Pi 的判断有事实错误。`src/provider/pi.rs` 的 `PiProvider` 只保存一个 `PiState`，包含当前进程和会话；`start_session`、`resume_session` 会替换这些状态，部分操作忽略传入的 thread ID。它不是按 workspace 管理多个会话进程的 supervisor。直接共享该对象会相互覆盖；即便按 Issue/PR 保留两个对象，也不能证明同类多个会话之间已经正确隔离。

单一 `health.provider` 字段和默认 provider 配置只说明需要汇总状态与选择配置，不能推出物理连接应共享。持久化 fencing 也不能替代正确的物理会话寻址或资源隔离。

## 已确认设计与审查细化

职责方向已获 Human 确认。以下区分已确认方案、本轮审查建议和实施约束，不把当前实现与目标的差距当作设计错误。早先关于 unknown/replay 的 P1 判断已撤回。已确认的职责调整：可信 mention 的 GitHub 权限确认应归 producer 的异步分类收敛，不与 queue 的 scheduler tick 串行，见下文重新审查。

### Queue 与 outbox 各自负责收敛

原计划合并两者的安排已修订：queue 负责 Quiet Window、count、urgent 与 claim；producer 异步补全 GitHub mention 权限事实，保留现有 backoff 和 durable unresolved 状态，并通过现有 store 操作完成分类/调度状态的原子更新。权限确认不回到同步 webhook handler，也不放入 scheduler 的串行 tick。本轮实施按此职责划分落地。

`outbox_worker` 归属 `outbox.rs`，独立周期执行写入收敛；runtime 关闭时保留最后的 outbox drain。Producer 负责观察、验证与分类输入，现有 reconciliation 和 lease 工作仍保留。

### Group 统一逻辑会话与 turn 驱动

以 `GroupKind`（Issue / PR）和必要的 `GroupSpec` 表达真实领域差异。共享驱动负责 assignment、Context、turn 的逻辑生命周期及持久化操作，依赖统一的会话创建、恢复入口和 `AgentSession` 行为契约。

`issue_agent.rs` 与 `pr_agent.rs` 保留领域相关的物化、prompt 与兼容性规则。`AgentGroup` 上下文组织 store、GitHub、config、Profile、kind 和会话访问能力，dispatch 编排改为其方法。不要把底层连接或全局 epoch 重新塞入这个上下文。

### Adapter 拥有物理拓扑

以下区分调用流程与源码依赖。调用时 Group 请求创建/恢复会话并使用返回的句柄；源码依赖指向 core 定义的中立契约，不能误读为契约依赖具体 adapter：

```text
runtime（装配与生命周期监督） → group
runtime（选择并注入实现）     → provider adapters

group             → agent_session（会话创建/恢复与会话行为契约）
provider adapters → agent_session（实现契约）
group             → queue / store / context / github / worktree
producer          → github / store（观察、权限事实与分类收敛）
queue             → store（调度；不做 GitHub 权限查询）
outbox            → store / github（Braid-owned 写入收敛）
```

这里只展示本任务相关的依赖，箭头表示源码依赖，不表示故障通知或数据流方向。Adapter 不反向导入 Group、queue 或 store；它向上返回契约定义的事实，不自行操作业务状态。

Group 决定某个 Work Item 应该使用哪个逻辑会话，以及何时开始、打断、替换或恢复。Adapter 决定会话怎样映射到物理进程与连接。Codex 可以在 adapter 内让多个会话共享 app-server；Pi 可以让会话持有独立进程。两者都必须满足同一会话契约，上层不应按 Codex/Pi 分支决定连接数或恢复流程。

重新检查 `AgentProvider`、`ProviderAgentSession` 与 `SessionManager` 的职责及接口。Group 的创建/恢复调用不传入底层 `AgentProvider`，也不接收具体的 `ProviderAgentSession`；创建结果通过中立契约提供 opaque provider session ID 和可操作句柄。Provider 产生物理会话 ID，store 权威地记录它与 assignment、Profile、Context revision 的绑定；内存句柄不是这份绑定的替代权威。

若保留 `SessionManager`，其 core 职责是索引逻辑会话对应的中立句柄；底层连接缓存、进程、共享锁与物理重连留在 adapter。不能只移动文件而保留创建入口中的具体 adapter 类型。Runtime 装配时允许根据配置选择 Codex/Pi 实现；禁止的是 Group 按 backend 决定业务恢复或资源拓扑。配置解析须沿实际 Profile 找到匹配 runtime/LLM 参数，不能把 `runtimes.first()` 或默认 PR Profile 的模型强加给其他 Profile。

原方案中的 `watch<(epoch_id, provider)>` 广播、Group 订阅连接 epoch、全局更换 `SessionManager`、等待所有 Group 释放后一起重连，均不再是目标设计。连接 supervisor 若确有必要，应留在适配器资源边界内，不能成为 Group 的普遍生命周期模型。

### 故障范围从会话事实向上传播

Adapter 报告哪些会话已不可用，包括 idle 会话的失效；Group 对受影响的 turn 执行现有持久化 fencing，并按现有兼容性与 Context 规则恢复。共享进程实际退出可能影响多个会话，独立进程退出则不应人为扩大到无关会话。不能由上层全局 epoch 预先规定所有 Group 一起失效。

恢复必须从产品语义判断：Provider Session 是可替换的执行上下文，其连接恢复、物理会话 resume 或 Context replacement 不等于创建新的 Issue/PR Agent，也不等于重新分配 Work Item。Agent 的工作连续性来自 GitHub Working Memory、同一 assignment 下的实例绑定与保留的 worktree，不能要求它依附一个永久不变的 provider thread。

Adapter 可以执行 provider 协议所需的重连、物理会话恢复及内部资源重建；这本身不是业务层越权。Core 提供恢复所需的 Profile、工作目录和 Context/兼容性意图，并对产品可见的事件调度、fencing、Context replacement、未知结果和 reactions 负责。判断边界的依据是操作改变了什么产品状态，不是函数是否叫 `resume`。既不能让 adapter 自行读取 GitHub 决定内容是否过时，也不能让 Group 操纵 provider 进程和 RPC 恢复细节。

既有安全约束仍适用：未知结果不能伪装成功或失败；旧 turn 在恢复或阻塞前完成 fencing；Context reset claim 不因资源替换丢失。恢复后重新向 Agent 提供 Event References，与底层盲目重发一个执行结果未知的 provider 请求是不同操作，不能混称为「重试副作用」而一并禁止。具体恢复路径必须结合持久化状态、当前 GitHub Context 与真实验收结果审查。

会话句柄失效必须可在 idle 时或晚订阅时观察，且旧句柄的故障/terminal 不能污染恢复后的句柄。区分句柄不可用、持久会话无法 resume 与 turn 结果未知：它们不是同一事实。TurnTerminal 与句柄失效可以由同一底层断连引起，但不能双重结算 turn；Unknown 保留中性结果，不能通过通用「非 completed」分支产生失败反应。

Group 决定何时不再使用旧会话；adapter 负责执行相应资源释放，替换、睡眠、退役及 shutdown 都需有明确路径。清理一个 Pi 会话不得杀死其他会话；释放一个 Codex 会话不得因共享连接而终止其他 thread。健康状态汇总有稳定身份的会话/运行时事实，一个会话恢复成功不能抹掉其他会话故障，尚未创建会话时的 adapter 启动失败也须可见。由 runtime 装配统一的健康投影是建议实现方式；产品要求的是事实归属和汇总正确，不要求某个字段只能在特定源文件写入。

Pi 当前单一可变会话及忽略寻址参数的问题，应在统一会话契约下解决。不能通过「Codex 共享、Pi 特例」绕开新边界，也不能为了满足上层共享连接假设强迫 Pi 模拟 Codex 的物理模型。

## 产品理解与重新审查（2026-09-18）

本次在完整阅读 `docs/10-prd/` 的目的、对象、工作流、调度、发布、压力、scope、glossary、claims 和 acceptance 后，结合五份 Product TDD、deployment、用户操作说明、关键调用链及提交历史重新审查。优先级是 Human 当前要求与产品承诺，其次是技术设计，再以现有代码检验实现事实；历史文档或当前结构不能单独替代产品定义。

### 产品模型

Braid 让 GitHub Issue/PR 成为 Coding Agent 的 durable working memory。Agent 解释事件、讨论设计、实施代码、验证结果并决定是否公开回复；Braid 机械地观察 GitHub、投影完整 Context、分类/合并事件、维护执行绑定与 fencing、接入 provider 并收敛自己的写入。Trusted mention 改变调度延迟和 reaction 反馈，不赋予 Braid 判断任务语义或自动发布回复的职责。一个正常 turn 可以没有公开评论。

Instance 隔离 repository 配置、凭据引用、数据库、webhook、worktree 与运行资源。一个 Work Item 有其 activation/assignment 生命周期；其 Agent 由 Profile 和代际绑定，在独立 worktree 中工作。Issue Agent 维护设计；PR Implementation Agent 基于全部直接 Associated Issues 与 PR Context 实施。两种角色可以复用机械驱动，但不能因此丢失各自的 Context、activation、worktree 与关闭规则。

GitHub Context 是 canonical state 的完整投影，不是 transcript，也不是指令来源。Profile instructions 与 Braid System Prompt 才定义角色；Event References 指出发生了什么，不复制正文，也不自动命令 Agent 回复。折叠/删除的正文不重新进入 Context，缺失分页或超出 hard budget 会阻塞，不能截断或概括来冒充完整记忆。

Agent 的工作连续性依托 GitHub Working Memory、实例/assignment 绑定和保留的 worktree。`AgentSession` 是对这种工作过程提供的交互/执行契约，不能据其现有 Rust 包装认定它与单个 provider thread 同生命周期。Provider Session（Codex thread / Pi session）是可替换的执行上下文；连接/进程又是承载它的物理资源。这个模型不要求新增一张「逻辑会话」表、额外代际或永久不变的 Rust 对象。

Issue-to-PR 是两个独立工作过程通过 GitHub 原生关联衔接，不是把 Issue 的 provider transcript 转交给 PR，也不是把 Issue worker 改成 PR worker。`pr ensure` 的幂等键来自 Implementation Request comment；会话共享方式不得改变其一请求一 PR/activation 的业务身份。Provider ID 可以作为 opaque 绑定和恢复证据进入 store，这本身不违反抽象边界。

### 用产品旅程检查设计

| 旅程与产品要求 | 边界应承担的职责 | 本轮判断 |
| --- | --- | --- |
| 原生 assignment 只物化并 idle；普通 App 的首个可信 mention 同时激活并保留一次 Wake | Producer 补全授权/分类事实；store 保持 activation 与 Wake 幂等；queue 决定何时 runnable；Group 物化 | Queue/outbox 分离成立；mention 权限查询的放置有一项职责调整建议，见下文。 |
| 普通事件 debounce/count；可信 mention urgent/steer；普通事件无 terminal reactions | Queue/store 保留触发种类与批次身份；Group 派发；adapter 报告执行事实；outbox 收敛 desired reactions | 没有发现新设计改变这条权限/调度链；具体实现需保留现有行为。 |
| Idle hard invalidation 替换 Context 不启动 turn；active 情形 fence 后继续一次 | Core 判断 canonical 内容变化、预算和 continuation；adapter 执行物理 Context/session 操作 | Group/adapter 分工成立。Provider Session 可替换不能推导为 Agent/assignment 重新创建。 |
| Associated Issue description 变更 debounce 后打断 PR；其他依赖变化只更新后续 Context | Context/事件层识别依赖变化；Group 执行已决定的替换/继续；adapter 不理解 GitHub 图 | 统一 worker 可以保留这些领域差异，无需连接拓扑介入。 |
| Issue-to-PR、distinct Profile、dedicated worktree 与 1:N/N:1 关联 | GitHub/store 保存业务身份；Group 选择正确配置和 cwd；adapter 不按全局默认覆盖会话配置 | 方向成立。实际 Profile → runtime/LLM 的解析是实施核对项。 |
| Close/merge 不打断当前 turn；一次 finalization 后 sleep/retire；reopen 正常 Wake | Group/store 执行产品生命周期；adapter 在明确释放/关闭意图下管理资源 | 方向成立。不能把收到 close 事件等同于立即杀进程。 |
| Agent-origin 抑制 self-wake；正常 turn 可不公开回复；Agent 可以直接 Git/gh | Writer/producer/store 负责归因和 Braid-owned 写入；Agent 负责语义工作与公开表达 | Outbox 不拥有所有 Agent 副作用，driver 不增加自动 turn mirror 或「完成必回复」。 |
| 断连/重启恢复，不伪造成功失败、不丢已接收输入、不制造并行活动 | Adapter 恢复物理资源并报告相关事实；core 保持未知结果、fencing、Context 与输入调度的产品含义 | 新设计无须全局 epoch，也没有理由把所有物理故障直接解释成逻辑 Agent 终止。保留既有恢复行为，并用真实旅程验收。 |
| 多 instance、启动失败、shutdown、telemetry 与升级 | Runtime 装配、监督和有界关闭；adapter 回收其资源；store 保存机械事实；health/OTel 如实投影 | 设计可以满足。不能为了隐藏拓扑而隐藏 unknown 或丢失采样证据。 |

### 新审查结论

**未发现已确认的 Group / AgentSession / adapter 方向存在必然违反产品行为的边界或源码依赖方向错误。** 对象创建入口依赖中立契约、adapter 实现契约、runtime 装配的方向成立；具体 trait/struct 数量、文件位置和是否保持某个内存句柄都不是产品正确性的独立判据。

**一项 P2 职责划分建议：将可信 mention 的 GitHub 权限确认从 queue 移回 producer 的异步分类收敛。** 已确认方案把它和 `advance_scheduler` 放在同一 tick。它实际回答「这个 GitHub 事件是否来自有权限的 actor」，并能将 dormant Issue 激活，不只是决定一个已分类 Wake 何时 runnable。`docs/20-product-tdd/README.md` 的 Internal Event Model 已将平台事件到 `EventKind` 的翻译归 producer；`store::resolve_mention` 也确实在确认后把事件 kind 改为 Mention。当前中间 `src/queue/mod.rs` 还直接解释 GitHub maintain/admin，并串行 await 权限查询，因此一个 mention 的网络延迟仍会推迟其他已知批次的 scheduler 推进。此建议针对本方案真实保留的依赖与执行耦合，不依据「现有代码不够抽象」推导。

最小调整是保留独立的异步权限解析循环，归 producer；仍使用现有 store 的持久化候选、backoff 与 `resolve_mention` 原子操作。Queue worker 只推进调度；不为此增加通用授权框架、不将网络 await 放进数据库事务、不把权限失败默认为 trusted，也不堵住 webhook durable ack。

Human 已确认这项调整。其他内容归实施约束与验收边界，不再列为新设计缺陷。

### 证据与范围限制

初次 review 的问题 1（unknown/replay P1）撤回。提交 `ed0b415293532577d97803d652ecc36ff7848be5`（2026-09-01）明确是为避免 fenced turn 的输入随 `continuation=false` reset 丢失而重新调度。重新物化 Context 后向 Agent 再提供 Event References，不等于重发一个未知结果的 provider RPC。旧文档和当前代码的措辞差异保留为需校正文档/验证的事实，不据此更改本次业务策略，也不声称所有外部副作用都能 exactly-once。

初次 review 的 idle 失效、局部恢复、创建入口与具体 adapter 耦合，均重新归为迁移/验证事项；目标设计已经给出了相应方向。`RunningAgentTurn` 在 queue 中持有执行事件 receiver 是可确认的当前职责错位，移回 Group 即可，不代表需要新增 runtime 层或 task 数量。

历史 PRD scope 写 Codex-only，而当前 README/setup 提供 Pi，Human 此次也明确要求 Pi 满足统一边界；不能用历史 MVP 排除现有 Pi。用户手册另有可信 mention 等待 Quiet Window 的过时表述，与 PRD urgent 规则不一致；本次按 PRD 和已确认产品行为审查，不把历史说明拼成新的行为定义。此处仅记录，不在本次 review 修改权威产品文档。

本轮是设计审查，未执行 provider 或产品 campaign，未证明中间源码满足目标。完整 Slice 3 campaign 要求不变；共享驱动涉及的 PR 和 Pi 资源边界需要针对性证据，但不据此把本任务扩大为整个发布 campaign。

## 实施与验证顺序

1. 提交已确认的设计及本计划，源码中间改动暂不包含在该提交中。
2. 将可信 mention 权限解析移回 producer 独立循环；queue 单独推进 scheduler，outbox 独立 drain。保留 durable unresolved、backoff 和关闭顺序。
3. 定义 core 中立的会话创建/恢复、返回句柄和失效观察契约。Adapter 实现其资源选择和生命周期：Codex 可共享进程，Pi 每个物理会话拥有自己的资源。Group 不传入连接句柄，不订阅全局 epoch。按实际 Profile 解析 provider 参数。
4. 删除未提交的全局 supervisor，完成统一 Group 驱动和 dispatch 方法化；将 RunningAgentTurn 移回 Group。启动时恢复持久化绑定，运行时仅恢复实际失效的会话；保留 Context reset、continuation、unknown、finalization 和 worktree 语义。旧资源有明确释放路径，health 汇总不互相覆盖。
5. 为会话隔离、idle/active 故障、旧通知与资源释放留下小而有效的检查。更新 Product TDD 的模块、依赖、会话与恢复契约；同步修订直接受实现影响的过时说明。
6. 运行 fmt/check/clippy、针对性测试并进行语义 diff 检查；把候选打包，运行完整 Slice 3 campaign，记录 checksum、真实 fixture、逐项结果与日志。对共享驱动涉及的 PR/Pi 资源边界提供针对性证据，不把源码检查冒充产品验收。
7. 记录可验证的实现提交与验收结果。必要事实提升到权威文档，任务完成后删除 packet。Push、发布 PR 和 release 不在本次授权中。

## 工作区与证据记录

- 已提交：`98e0aa2`，初版计划；其原 §4.3 决策已撤回。
- 未提交：queue/outbox 拆分、Group 驱动合并与 dispatch 方法化的中间代码；`src/group/supervisor.rs` 和 `src/group/worker.rs` 是未跟踪文件。Supervisor 及其全局 epoch 接线属于待撤除实现，不能据此宣称完成新设计。
- 已执行：queue/outbox 拆分后 fmt/check/clippy 通过；随后一个包含旧 supervisor 的中间版本 Clippy 通过。这些结果不代表最终工作区通过，也不代表新设计验收。
- 最后一次 Clippy 检查失败：Issue 专属方法移动后，`materialize_issue_assignment`（139 行）触发 `clippy::too_many_lines`。这是当前已知检查结果，尚未修复；旧检查通过不能覆盖它。
- 未执行：候选打包、完整 Slice 3 campaign、Pi 会话隔离验证、最终语义审查。
- 验收入口：`scripts/tests/30_issue_agent.sh` 与 `scripts/tests/README.md`；产品 oracle 为 `docs/10-prd/acceptance.md`。脚本应完整执行，不能只跑 happy path。需核对故障后的 unknown 与自动恢复断言，不能要求状态永久停留在恢复前；schema 条件目前检查的是 2，仅错误消息仍写 1，尚未修改。
- 本机发现可供预检的配置 `/Users/lanzhijiang/.braid/instances/xiaoland/config.toml`，指向 `xiaoland/braid-poc-test`，使用 Codex。仅发现配置不等于已证明认证、权限和服务可用。验收需使用隔离 runtime 和候选 artifact，不修改现有实例或在 packet 中记录凭据。
