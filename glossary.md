# Braid Glossary

| 术语 | 含义 |
| --- | --- |
| Braid App | 安装到 GitHub 的 App、mention target 及稳定 Bot 写入身份；不是 Coding Agent。只有被 GitHub 专门 provision 为 Agent App 的 installation 才能成为原生 Issue assignment target；普通 GitHub App 不能通过标准 assignee API 被 assign。 |
| Work Item | 一个由 Braid 激活的 GitHub Issue 或 PR。 |
| GitHub Issue | Issue 在 Context 中的 repository-qualified 对象引用，例如 `owner/repo#123`；不是 Markdown link，也不另列重复 URL。 |
| GitHub PR | PR 在 Context 中的 repository-qualified 对象引用，例如 `owner/repo#456`；不是 Markdown link，也不另列重复 URL。 |
| Associated Issue | 与一个 PR 建立 GitHub 原生关联、因而向该 PR 提供 Issue Context 的 Issue。一个 PR 可以关联多个 Issue，一个 Issue 也可以关联多个 PR。 |
| GitHub Context | 从当前 GitHub canonical state 确定性渲染出的、面向 Agent 的简洁 Markdown 或纯文本；不是 JSON、provider transcript 或创建时快照。 |
| GitHub Working Memory | GitHub Context 作为 Agent 的 durable working memory 的产品语义。Agent 通过更新 description/body、metadata 和简短 comments 管理未来 Context，而不是依赖不断膨胀的 provider transcript。 |
| Issue Context | GitHub Issue、title、description、纳入范围的 metadata，以及 comments 按生命周期规则组成的 GitHub Context。 |
| PR Context | 依生命周期规则投影一个或多个当前直接 Associated Issues（open 为完整 Issue Context，closed 仅 reference/metadata），随后是当前 PR 自身的最小实现上下文。 |
| Context Materialization | 从 GitHub 重新读取 canonical state，并为一次 turn/reset 生成 GitHub Context。 |
| Context Revision | 一次 Context Materialization 的机械版本；只用于 freshness、fencing 与幂等，不作为 JSON/debug 内容展示给 Agent。 |
| Provider Context | Codex、Pi、Claude Code 等 provider 实际交给模型的 context window/history。 |
| Context Replacement | 让 Provider Context 对齐当前 GitHub Context；必要时会替换物理 Provider Session。 |
| Provider Compaction | Provider 自己的有损 compact；不是 Braid 的 GitHub Context，也不能替代 Context Materialization。 |
| Event Reference | 作为 user message 发送给 Agent 的简短 Markdown/纯文本引用；包含发生了什么以及可读取目标，不复制正文、payload 或 JSON。 |
| Wake Event | 进入 debounce 队列，并在 Quiet Window 或数量阈值满足后启动 turn 的外部事件。 |
| Hard Invalidation | 非 Agent-origin 的变化改写或移除了同一 Work Item 已经进入 Provider Context 的事实。它立即 fence 旧 Context Revision 的写能力，并要求在安全边界 Context Replacement；自身不一定启动 turn。 |
| Dependency Dirty | PR 的直接 Associated Issue 发生非 description 变化。它不打断当前 PR turn；Braid 在下一次 PR turn 前重新 materialize PR Context，并按规则投递 Event Reference。 |
| Cross-surface Hard Invalidation | 非 Agent-origin 的直接 Associated Issue description 变化。Braid 先按 debounce 聚合，窗口结束后 fence/interrupt 当前 PR turn，并用最新 Issue Context 替换 PR Provider Context。 |
| Agent-origin Event | 由当前 Agent 通过 Braid 写入关联，或由该 Agent Profile 明确配置且经 GitHub node identity 验证的独立 Agent actor 产生的变化；进入未来 canonical Context，但不反向唤醒或 reset Agent。无法可靠归因的直接写入按外部变化处理。 |
| Agent Profile | Braid 管理的不可变 Agent 配置，包含 provider、model、reasoning、user instructions、tools、skills、MCP 等。 |
| Profile User Instructions | Agent Profile 提供的行为、风格与专长指令。它们在创建 Provider Session 时加入 Effective Agent Instructions，并与 Braid 的工作记忆、角色和协作说明共同构成实际指令。 |
| Profile Tag | Agent Profile 声明适用面的 tag-like 类型，例如 `issue`、`pr`；同一 Profile 可同时拥有多个 tag，也可以只适用于 Issue。 |
| Braid System Prompt | Braid 在创建 Provider Session 时注入的高优先级、版本化指令，包括 Braid/CLI 的存在、GitHub Working Memory 协议，以及按 Issue/PR surface 选择的角色与任务说明。它帮助 Agent 使用产品能力，而不是把 Braid 变成限制 Agent 的权限沙箱。 |
| Effective Agent Instructions | Provider 实际收到的指令组合：Braid System Prompt 加 Profile User Instructions。GitHub Context 是带来源边界的不可信工作数据，Event Reference 是 user message，两者都不是系统指令。 |
| Issue Agent | 一个带 `issue` Profile Tag、运行在某个 Issue 上的 Agent 实例。 |
| Issue Agent Group | 同一 Issue 上所有平行 Issue Agents；没有 primary/sub-agent，收到相同 Context 与 Event Reference batch。 |
| Issue Group Turn | 同一 Context Revision 和 Event Reference batch 并行扇出给一个 Issue Agent Group 的一次 turn。 |
| PR Agent | 一个带 `pr` Profile Tag、运行在某个 PR 上的 Agent 实例。v1 只有 Implementation Agent；未来可增加 reviewer、advisor 等角色。 |
| PR Agent Group | 同一 PR 上的 Agent 集合。v1 恰好包含一个 Implementation Agent；架构保留未来增加非实现角色的路径。 |
| Implementation Agent | PR Agent Group 中负责修改代码的 Agent。v1 每个 PR 恰好一个，并独占一个专用 worktree。 |
| Implementation Request | Issue Agent 根据某条 Issue comment 发起的一次实现请求。该 GitHub comment ID 是 `braid pr ensure` 的幂等键；同一 comment 只得到一个 PR，不同 comment 可得到不同 PR。 |
| PR Activation | 启动一个 PR Agent Group 的机械事实。产品上等价于把 PR 交给 Braid；具体 GitHub signal 由 adapter 提供，不能在未验证前假定为原生 PR assignee。 |
| PR Agent Lease | 将一个 PR、一个专用 worktree 和一个 `pr`-capable Profile 原子绑定给唯一 Implementation Agent 的独占租约。 |
| Finalization Turn | Issue close 或 PR merge/close 后允许对应 Agent Group 额外运行的、至多一次的收尾 turn。它处理已聚合的 terminal lifecycle 事件，然后令 Agent Group 休眠或退役。 |
| Provider Session | Codex thread、Pi session、Claude session 等物理会话；可替换，不是 canonical collaboration authority。 |
| Assignment Generation | Work Item 连续由 Braid 激活的一个生命周期；Issue 可由经 canonical 确认的原生 Agent App assignment 开始，或在该能力不可用时由 dormant Issue 的首个 Trusted Braid Mention 开始；PR 由经验证的 PR Activation signal 开始。重新激活会 fence 旧 generation 的能力和输出。 |
| `braid gh` | 使用稳定 Braid App 身份执行 GitHub 写操作、并尽量保持 `gh` 交互习惯的 CLI。MVP 只实现写侧子集，因为读取仍可使用 Agent 自己的 GitHub CLI；未实现的命令是实现范围，不是产品层禁止。Agent 仍可直接使用 `gh` 与 `git`。 |
| Memory Commit | Agent 修改 description/body、metadata、comment lifecycle 或发布简短 comment，使结果进入未来 GitHub Context 的写操作。它可以通过 `braid gh` 或 Agent 已有的 GitHub 能力完成。 |
| Write Intent | 在 Braid 自己执行 GitHub mutation 之前写入 SQLite 的不可变操作意图，记录目标、期望操作、幂等/关联信息与 origin。Agent 直接执行的 `gh`/`git` 操作不经过该机制。 |
| Write Outbox | 在单 owner lease 下执行、查询并收敛 Write Intents 的 durable 状态机。终态为 applied、conflict、ambiguous、rejected 或 superseded；timeout 只产生 uncertain，不等于失败。 |
| Quiet Window | 防抖窗口；与累计事件数量阈值共同决定何时启动 turn，可信的可见 mention 可以绕过两者。统一不用 `quite window`。 |
| Trusted Braid Mention | repository maintainer/admin 在可见 Markdown prose 中对配置 handle（默认 `@braid`）的精确 mention。它绕过 debounce；普通 write/triage/read actor、code、quote、HTML comment 和 Braid-origin 内容不具备该能力。 |
| Agent Attribution | Coding Agent 自己发布公开评论时使用的、面向 Human 的 quote-block Profile/角色标识。稳定 GitHub actor node identity 是 Agent-origin 的首选依据；在独立 actor 尚未配置时，精确 attribution 只作为防止评论回灌的弱相关线索，不授予权限。Braid 不镜像 Agent turn。 |
| Operational Status Comment | Braid 自己维护的、与 Agent comment 分离的可见状态投影，例如 `context-too-large`。目标 surface 由 Agent Profile 配置；Braid 按 profile/surface/generation 更新一个稳定 comment，并从 GitHub Context 中排除它。 |
| Reaction Lifecycle | Braid 在 comment 上维护的轻量反馈：所有新 comment durable ingest 后可加 `eyes`；只有 Trusted Braid Mention 所启动的显式 turn 才在该 comment 上使用 `rocket`、`+1`、`confused` 表示 active、正常结束、异常结束。普通 debounce turn 不展示 turn lifecycle reaction。 |
| Telemetry | Braid 通过 OpenTelemetry 产生的 traces、metrics 和 logs。metrics 全量记录；完整 traces/logs 默认在 root 做一致性的 10% head sampling，并可按 repo/profile/事故窗口切到 100%。若需要按最终异常结果保留 100%，由可选的外部 Collector 做 tail sampling。被采样证据允许包含评论正文与摘要、credentials、provider transcript、原始 webhook payload 和本地路径；采样控制数据量，不是脱敏或保密边界。 |
| Local State | Braid 为路由、幂等、调度、租约、outbox、provider handle 与 schema state 保存在 SQLite 中的机械事实；不是 GitHub Context 或 provider transcript 的 authority。 |
| Database Migration | 随 binary 发布、单调编号且发布后不可修改的 forward-only SQLite schema/data 迁移。不存在生产 down migration；回滚应用版本前必须先验证数据库兼容性。 |
| Context Pressure | 完整 GitHub Context 接近某个 Profile 的可用 provider window 时产生的状态。soft pressure 要求 Agent/Human 整理工作记忆；hard limit 禁止用残缺 Context 启动 turn。 |
