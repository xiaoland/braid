# Pi 运行栈反哺实施结果

2026-10-11，用户“实施这些遗漏”授权的源码融合已完成。基线 `2808c89`，目标
Pi 1.0.2；实际反馈来自编译、原生 CLI/RPC/SDK 与本轮自身的进程操作。
[此前 MiniMax 双上游验收](../real-model-e2e/results.md) 的整体未通过结论保持不变。

## 交付与边界

`native/pi/` 交付锁定依赖、插件选择、六组有序语义补丁和独立入口。CLI 与 RPC
使用同一安装，固定 Node；材料清单包含 28,639 个文件及文件链接入口身份，禁止
链接逃逸安装目录。Rust 在启动前校验 SHA，实际启动再核对 execution ID、能力、
真实扩展集合及 loader errors。setup 识别相邻 manifest，doctor 复用同一校验器。
外部基础 Pi 保留，宿主自动扩展发现关闭，不修改系统全局 Pi。

迁入的共同合同包括 awaited hooks 之前的唯一重试意图、extension_error 与取消结果、
结果交接、Braid 所有的推理、subagents 所有权／wait／候选排除边界，以及 PBB
精确 handle／有限任务和服务。异步子执行必须等待真实 child startup，不能先凭
runner PID 报成功；Node launcher 与真正 writer PID 分开，lease 使用实际 payload。
所有出生身份消费者在 Mac 上统一查询，稳定 boot 使用 boot session UUID。

每次物理执行重新分配身份，先登记再执行，在共同 admission 锁内 fence 新启动。
关闭依次收口 owned 后代、EOF／父 wait、精确全 execution 离线清理；停止未知
阻断替代执行。Core turn idle 不代表后台结果已消费；service handoff 不能越过
有限义务。持久化终态之后的失效通知仍保留诊断。

21 项历史输入按语义融合：PBB 与 subagents 的共同生命周期、core 边界及 managed
接线、pi-ai connection-reset、Context7 HTTP 诊断和 MCPorter owned service 纳入。
可选 compaction threshold 不设 Factory 默认值。Factory acceptance gate 删除、
固定模型／角色、资源压力政策，以及 FFF／其它工具描述删改不迁。
技能保持独立文件，不内联到提示。

生产合同归 [provider 技术说明](../../docs/20-product-tdd/app-server.md)，
安装与处置方法归 [原生材料说明](../../native/pi/README.md)。

## 实际反馈

最终安装为 `/Volumes/WorkSSD/Development/braid-native-feedback/installed-v11`。
manifest SHA-256 为 `612fea8c884b6e3a75f0ddf84ab09008216b15dd2fa4f573b8d07ec0dba77c48`。
managed 模块 SHA 为 `29d49404ea98a4c89da0dc0ba28d6465f1aeb5527f47e50ac37fa2cd6ed9fd8c`，
process-owner 为 `203f39fd4da47b6539ac95ade9e3a094ebe95bd4daae6d1e33eb623ac91053ba`。
源码采用清单为同证据根的 `native-source-final-v11.json`。

原始回执全部位于 `/Volumes/WorkSSD/Development/braid-native-feedback/`：

| 证据 | 观察与实际限度 |
| --- | --- |
| `operations/rpc-v11/` | 四个选用扩展实际加载，errors 为空、execution ID 准确、quiescent，进程 exit 0。 |
| `operations/sdk-pbb-v11/` | 真实 finite 命令 busy→wait/result，保留 exit 7 和日志；错 handle 明确失败；service busy→handoff quiescent，收口 stopped。 |
| `operations/subagent-startup-v11/` | 返回前 child 确实 started；随后 busy→自然 wait→保留失败与产物→cleanup stopped。无凭据子调用保留 `No API key found for anthropic`，没有模型 HTTP 请求；不是 MiniMax 配方验收。 |
| `e5/operations.json` | 极短真实 `/usr/bin/true` 的 startup 与 code 0 close 均保留。 |
| `e14/operations.json` | A 的 launcher 被 SIGKILL 后 payload 仍存活；离线收口 A，B 的原 PID、birth 与 PGID 继续存活，再独立收口 B。 |
| `e16/operations.json` | 稳定 boot session UUID 下实际 shell/sleep busy→全 execution stopped，真实 SIGTERM。 |
| `operations/braid-cli/doctor-v11.json` | Braid 的材料和 version 检查通过。隔离配置没有 source checkout／OTLP，整体 ready=false；placeholder 只证明 key 存在，不证明供应商认证。 |

中央 `cargo check --locked`、`cargo clippy --locked -- -D warnings`、
`cargo build --locked`、`cargo fmt --check` 与 diff 空白检查通过。

早期失败均保留，没有覆盖成成功。v5 的 await 传播遗漏导致实际加载 parse 错误；
v6/v7 暴露 Mac lease 的 Linux-only 查询、writer 与 launcher PID 混淆及退出期间
marker 竞态；v10 暴露 boottime 随校时漂移。它们分别通过完整调用传播、实际 payload
登记、Mac birth 查询、退出后的 OS 回读及 boot session UUID 修复。
仍存活的身份冲突继续 unknown；旧 v10 unknown 不被较晚无人存活回读追认为当时成功。

## 未取得的反馈

没有运行 Braid/Factory 测试或模拟设施，没有重跑付费模型或旧运行。
真实 subagent 推理、自然模型重试、跨 turn 未消费结果 drain、owner 恢复的模型路径
及 Linux 现场仍未验。Context7／FFF／MCPorter 的装配及加载不证明外部工具请求
旅程通过。当前源码融合不能替代下一轮冻结插件配方的真实模型双上游验收。

Mac 身份锚与标记都不能确认时保持 unknown。此合同不是安全沙箱，不能撤销已发出的
外部请求。SDK 的 root-descendants 回执不等于全 execution 停止；父退出后的全范围
证明另取 e14，不能仅靠父 exit 0。

主 Agent 首次中央 Cargo 命令漏设 WorkSSD cache/TMPDIR，写入系统 `~/.cargo`
缓存后中止；后续命令全部明确使用 WorkSSD。保留此存储偏差，不宣称本轮全项合格，
不擅自删除共享全局缓存。独立 Braid 仅本地提交，未推送或发布。
