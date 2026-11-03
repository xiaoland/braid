# Open Agent Runtime 接入调查

2026-10-12。调查基线为 Braid `50f25c87` 与 OAR `7a981975f7aa5beb3fa33b54eecfcc3b471e349e`（包版本 0.59.1）。用户随后批准“先来收敛 pi 交付”，当前实施范围为本文建议的直接 Pi 公共 SDK 路线；没有批准接入 OAR 或付费模型验收。下文 OAR 证据来自只读源码调查，没有运行 OAR。

## 要解决的问题

Braid 当前不仅适配 Pi RPC，还维护了一份经修改的 Pi 和插件发行材料。`native/pi/install.mjs` 对六个锁定包逐项应用补丁；未选择的插件同样安装并被修改。补丁哈希和发行 manifest 能保留身份，却不能减少跟随上游 bundle、内部接口和插件行为的维护成本。

补丁不能按文件整体判定去留。Pi 核心补丁混合了会话错误、retry、settled 和 compaction 行为，以及 Braid 的执行身份、后台服务交接、材料发现和停止协议。插件补丁也同时包含通用正确性修复及 Braid 运行策略。交付目标应是明确维护边界、使用公开接口、减少私有依赖变体，而不是减少可见的 patch 文件数量。

## OAR 的实际边界

OAR 是 ESM Node >=24 的库，提供统一的 session/control/records 契约。普通 Pi runtime 内嵌 Pi SDK 1.1.0；不是对 Pi CLI/RPC 的包装，也没有独立 Pi runtime 子进程。原生 `agent_settled.aborted` 已有权威取消事实；OAR 的 accepted 控制回执与 turn 终态分开，原始 SDK 事件仍可读。这与 Braid 的输入受理、终态和 OTLP 事实边界相容。

普通 Pi 的环境、主题、HTTP dispatcher 等存在宿主进程全局状态。`SessionOptions.env` 影响 Bash 子进程及 stdio MCP，不是 provider key/base URL 配置渠道。模型和凭据使用原生 agent-dir/model 配置；`OAR_PI_AGENT_DIR` 是进程级设置。故若采用，应为每次 Braid 物理执行启动独立 Node bridge，不把不同 Profile 的 Pi session 混在一个长期共享 Node 宿主。

OAR 的 dispose 会先 abort 活动会话，再执行 extension shutdown hooks；hooks 超过十秒只报告 warning 并释放 session，不能证明扩展代码及外部后台进程全部退出。Braid 的出生身份登记、停止核对、后台结果交接和关闭失败阻断替换仍需明确的 Braid 所有权实现。简单使用进程组也不能覆盖已观察到的 detached 服务。

材料接线目前有具体缺口。OAR 的 Pi opener 默认加载原生资源，公共 SessionOptions 不提供通用 ResourceLoader、extension factory 或工具注册接口；文档也明确没有通用 allowlist/extension callback。进程隔离能隔离配置，却不自动证明选定 skills、角色及插件白名单，也不能自动将 Bash/subagents/background 的启动路径接入 managed owner。必须核实原生 settings 能否完整实现这些合同，或取得上游公开接线点；不能在引入 OAR 后再修改它的内部 opener 作为常规方案。

Pi Durable 是另一条执行架构：宿主提供已经打开的 Harness、storage、models 和环境，OAR 借用它。dispose 仅停止观察，不关闭 Harness 或运行。它不能替换当前 Pi CLI 插件及 Braid 物理执行关闭协议；本次不建议借机改成这套持久执行架构。

## 建议的职责和补丁归属

| 职责 | 目标归属 | 迁移条件 |
| --- | --- | --- |
| 原生 session、model/effort 生效、prompt/steer/abort、事件翻译 | 优先评估 OAR provider adapter | 公开接口可承载 Braid 材料与工具接线；无须复制或补丁修改 OAR 内部实现。 |
| GitHub/内建平台、Issue/PR 协作、调度、writer 权限、持久输入、Console/OTLP | Braid Rust core | 接入 runtime 不改变这些职责。 |
| 独立执行目录、进程登记、后台作业/服务、owned-stop、关闭失败阻断 | Braid integration 与执行监督 | Bash、subagent 和后台启动必须实际接入；abort accepted 或 dispose 返回不是停止证明。 |
| Profile 选定 skills/native_template、模型配方和材料 digest | Braid 配置及快照 | 默认资源发现不得绕过选定材料；启动和恢复核对实际生效值。 |
| 通用 Pi retry/error/settled/compaction 修复 | Pi 上游或已修复版本 | 逐项核对 SDK 1.1.0 的具体行为，不能由取消 flag 推断其余问题已修复。 |
| subagents/PBB 结果 drain、owner 丢失、重试及错误路径 | 插件上游；Braid 接线独立维护 | 明确区分插件正确性和 Braid 策略；原生角色不是 Braid Issue/PR 成员。 |
| Context7 错误正文、pi-ai 连接错误 retry | 对应上游或明确的可选集成 | 可选工具不再成为基础 Pi runtime 的必带补丁。 |

拟采用的边界是 `Braid Rust provider → 每次物理执行的 Node bridge → OAR → 原生 runtime`。bridge 只传递控制、原始 records 和身份，不引入第二套协作调度或 Agent loop。Braid 自有生命周期仍是交付合同。OAR 的会话内存队列不替代 Braid 持久 inbox；每次 resume 的 OAR seq 从零开始，记录必须包含物理执行身份和 native session ID。

## 实施前必须收敛的判断

当前推荐先升级 Pi SDK、收敛现有补丁，暂缓把 OAR 加入交付依赖。advisor 独立判断与本轮源码证据一致：OAR 当前公共接线缺口会迫使 Braid 在另一层维护私有变体，收益尚不足以抵消成本。先直接使用 Pi 公共 SDK 拆出 Braid integration，逐项删除已有原生支持的补丁，并上游通用修复。这条较小路线以消除 dist 修改为目标；插件缺陷尚未上游时仍明确保留补丁身份，不能承诺升级即全部删除。

OAR 的采用闸门是一个公开接口组合能同时实现隔离配置、选定资源、扩展/工具接线和进程所有权。以后确认可行，再接入一个 Pi provider 候选，并逐项退役等价职责的旧实现；迁移期间保留旧冻结发行用于对照和恢复，不作为永久并列 provider。若任一项只能通过修改 OAR 私有源码实现，就继续暂缓采用。未实际验证 Pi settings 是否足以补齐缺口，不将其作为已成立前提。

后续实现应先形成锁定 Node/依赖及许可证材料、通过编译和公开实际操作核对，再按用户批准的范围执行真实 MiniMax。必要实际反馈包括：显式资源选择、根/子模型一致、同步和异步子任务结果、有限后台结果及服务交接、活动取消、异常关闭及 detached 后代清理、历史恢复和材料漂移阻断、原始事件及终态完整到达 OTLP。不得新增设施测试、自检或包 smoke；旧 ARC 失败结论不因 runtime 替换自动改变。

## 证据入口

- Braid：`native/pi/install.mjs`、`patches.json`、`patches/`、`managed.mjs`、`process-owner.mjs`、`src/agent_session.rs`、`src/provider/pi.rs`、`pi_runtime.rs` 与 `src/profile_materials.rs`。
- OAR：[Pi 契约](https://github.com/botiverse/oar/blob/7a981975f7aa5beb3fa33b54eecfcc3b471e349e/docs/runtimes/pi.md)、[opener](https://github.com/botiverse/oar/blob/7a981975f7aa5beb3fa33b54eecfcc3b471e349e/packages/oar/src/runtimes/pi/open.ts)、[shutdown](https://github.com/botiverse/oar/blob/7a981975f7aa5beb3fa33b54eecfcc3b471e349e/packages/oar/src/runtimes/pi/lifecycle.ts)、[Pi Durable](https://github.com/botiverse/oar/blob/7a981975f7aa5beb3fa33b54eecfcc3b471e349e/docs/runtimes/pi-durable.md)、[宿主职责](https://github.com/botiverse/oar/blob/7a981975f7aa5beb3fa33b54eecfcc3b471e349e/docs/design/system.md)。
- 只读上游源码保存于 `/Volumes/WorkSSD/Development/braid-runtime-evaluation-20261012/oar`。主 Agent 拥有接入调查，profile_feedback_owner 完成现有补丁与生命周期责任梳理，autonomous_mode_advisor 提供路线判断。没有改变原验收资格或启动任何旧运行。
