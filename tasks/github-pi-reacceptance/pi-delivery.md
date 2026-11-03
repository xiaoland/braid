# Pi 交付收敛记录

2026-10-12，用户授权“是的，我们先来收敛 pi 交付”。本轮完成直接 Pi 公共 SDK 路线，不接入 OAR 或 Pi Durable，不启动付费模型、旧验收或生成应用。旧运行材料及原失败证据保留。

## 交付变化

Pi 和同系列依赖锁定到 1.1.0。`pi.mjs` 准备隔离环境和所选材料；自有 `sdk-host.mjs` 使用根包公开 SDK 构造 runtime、资源及受管 Bash；`rpc-adapter.mjs` 复用公开 RPC 实现，仅增加 Braid 的执行停止、服务交接和实际身份。JSON/text 子 Agent 复用公开 print 实现，保留长任务 `@file`、stdin 和图片附件接线。当前未支持的原生 CLI 参数明确拒绝，受管入口不是完整交互式 Pi CLI。

旧的 296 行 Pi 核心综合补丁已删除。会话生命周期、资源边界、CLI/RPC bundle、Bash 登记和 Braid 自主推理策略不再通过修改那些上游文件实现。基础发行仍保留 60 行提前压缩补丁，以及 pi-ai 连接重置分类补丁；前者尚无等价公开设置，后者保留实际错误正文对应的重试行为，不能为零补丁撤掉已有能力。

插件依赖用独立 npm workspace 声明，共用一份 lock。安装器只安装明确选择的插件、应用对应补丁，manifest 只声明实际安装和修改的材料。subagents/PBB 最后交接迁到公开 `agent_before_settle`，不再依赖私加的扩展 `willRetry` 字段；PBB 只在明确选择 subagents 时动态导入它的 API。插件内部正确性及 owned process/service 接线仍保留局部补丁。

自有 host 对最终模型错误保留未消费的 steering/follow-up 输入，通过原生 custom entry 保存，并在下一次明确输入前恢复；终态扩展 hook 错误请求公开 abort、保留错误触发停止的事实。原始取消标记不改写，RPC 扩展错误仍交给 Rust 优先判 failed。模型 Bash 和公开 RPC/user Bash 均接入同一受管实现，扩展明确提供的自定义 operations 保持原义。

## 已取得的反馈

Mac arm64 上按低并发执行，全部安装、cache、HOME、临时、运行记录和日志位于 `/Volumes/WorkSSD/Development/braid-pi-delivery-20261012`。

| 范围 | 实际反馈 |
| --- | --- |
| Rust | 单任务离线 fmt/check、Clippy `-D warnings`、build 通过；最终 check 5.06 秒、Clippy 6.43 秒、build 35.97 秒。 |
| JavaScript | 四个自有模块通过 Node 语法编译，diff 检查通过；没有测试、自检或包 smoke。 |
| 基础安装 | 124 个依赖包；五种可选插件均未安装，manifest 包含两份基础补丁。 |
| 独立 background | 实际安装、加载并取得命令清单，无 subagents 依赖或 loader error；读回 M3/medium、空闲状态和停止回执。 |
| subagents/background | 134 个依赖包，只加载对应两个扩展；Context7、FFF、MCPorter 未安装，manifest 包含四份实际补丁。 |
| 最终基础及双插件发行 | 顺序完成公开 new_session、get_state、get_commands、RPC Bash `pwd`、get_entries、服务交接和 owned-stop，全部响应成功。实际 SDK 为 1.1.0，模型为 minimax/MiniMax-M3、reasoning 为 medium。 |
| Bash 与关闭 | 保存真实 root/pi-bash 出生登记、Bash 历史和子树 stopped 回执；父进程 exit 0、stderr 为空，随后离线 execution cleanup stopped。 |
| doctor | 最终安装的 Pi native material 与 Pi provider 检查通过。整机 ready=false 的唯一剩余前提是 OTLP Collector 未启动；本轮没有为使报告变绿启动服务。 |

两份最终冻结 manifest：基础 `f7783a0e7ca734323960e8f87ba0a580bf1ccc463c717249efb2164d764090e6`；双插件 `5919c9347afcf2d5814eb4899af3815b2c33f7b75c69030b025811c08aa27a29`。raw/final-installations.json 保存完整安装选择、实际文件身份及停止回执；raw/final-base-rpc.stdout.jsonl 与 raw/final-both-rpc.stdout.jsonl 保存完整原生操作响应。两次操作均无 agent_start，不存在模型生成 turn。

## 原失败与资格边界

最初 Rust check 暴露 manifest 字符串部分 move 后再借用，修复后编译及严格 Clippy 通过。第一次基础原生启动暴露可选 `Args.extensions` 未初始化，修复为官方可选数组消费并保留失败安装。一次手工启动没有传与 Rust 一致的 launch-start marker，控制正确返回 unknown；修正行政操作的启动环境后重新取得 stopped，未放宽归属核对。所有这些旧执行均完成离线 cleanup，原失败没有被后续成功覆盖。

公开操作同时纠正了 live 身份的来源：版本取实际加载的 SDK VERSION，能力来自 host 实现，不能以 manifest 回显证明能力。采用检查还补齐 RPC/user Bash 接线、长任务文件输入和终态 hook failure 门控。这些修正没有改变收费范围。

本轮没有模型调用、同步/异步子 Agent 推理、实际重试/压缩、错误后队列恢复或终态 hook 故障实验。JSON/print 长任务和图片接线取得源码及编译反馈，没有取得模型执行反馈；Linux/x64 和其它可选插件也未实际运行。不能将安装、行政操作或编译通过当成真实模型端到端验收；旧 ARC/GitHub 资格结论不变。

持续的安装与操作说明归 [native/pi/README.md](../../native/pi/README.md)，跨组件运行合同归 [provider 技术说明](../../docs/20-product-tdd/app-server.md)。OAR 保持候选地位，其采用判断保留在 [调查设计](oar-integration-design.md)。
