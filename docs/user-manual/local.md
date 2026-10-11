# 内建协作平台

GitHub 与内建协作平台是 Braid Harness 的两种上游。内建平台独立持有仓库、
Issue、PR、正文、评论、指派、事件和候选合并记录，不依赖 Harness 的 Profile、
会话或执行数据库。停止 Harness 不停止平台，也不删除协作内容。Web Console
属于整个 Braid，可同时阅读平台、Harness 和 OTLP 观测。

## 声明平台与 Harness

先准备有命名分支的真实 Git checkout。平台有自己的配置与数据库，不需要安装
Agent CLI 或提供模型凭据：

```shell
braid platform init --source /absolute/path/to/repository --config /absolute/path/to/platform.toml --name demo
braid platform serve --config /absolute/path/to/platform.toml
```

平台声明固定 repository、source、默认分支、数据库与 loopback listen 地址。
默认分支不会随 source 当前检出的分支改变。平台对象只能访问已绑定的 Git
对象库，HTTP 调用不能提交任意文件路径。

Harness 单独声明上游与执行环境。准备实际安装的 Pi 或 Codex，并明确模型与
API key 环境变量名称；初始化不读取或保存 key 值：

```shell
braid init --source /absolute/path/to/repository --config /absolute/path/to/harness/config.toml --name demo --endpoint http://127.0.0.1:18082 --adapter pi --executable /absolute/path/to/pi --provider deepseek --model deepseek-chat --api-key-environment DEEPSEEK_API_KEY
braid config check --config /absolute/path/to/harness/config.toml
braid migrate apply --config /absolute/path/to/harness/config.toml
braid serve --config /absolute/path/to/harness/config.toml
```

Harness 的 `[builtin]` 只选择平台 repository 与 endpoint，与 `[github]` 互斥。
Profile 的 workspace 是执行源，与平台声明分别维护；首版候选交接要求两者
能访问同一个 Git 对象库，不承诺远程 clone/push 托管。平台不读取 Harness
数据库；Harness 通过 HTTP 获取当前对象和稳定事件，在自己的 ledger 保留
消费游标与去重记录。更换执行实例不会消耗或删除平台事件。

## 工作项与候选

通过[Web Console](console.md)可以不启动 Harness 就创建和讨论工作项。指派的
actor 是平台事实；Harness 将它映射为当前支持的 Profile。只有实际开始执行
并取得写入授权后，Agent 才能发布内容。对象正文是当前工作依据，评论是讨论
数据，不能获得系统指令权限。

`braid object --config <Harness配置>` 是 Harness 的对象接入入口，使用平台 API，
不直接修改平台数据库。运行中的 Agent 调用时，Harness 核对专属 cwd、当前
assignment、session 与 turn，并使用已授予的 writer grant；平台提交事务核对
授权版本、对象 revision 与 request ID。撤销授权先于替换执行，过期请求明确
拒绝。此合同防止正常执行串写，不是恶意本机账户的安全沙箱。

失去响应时，先用 `braid object --config <Harness配置> receipt <request-id>` 查询
原回执，不生成新请求。需要重复提交时保留原 `--request-id`、命令内容和
`--expected-revision`；不能重新读取当前 revision 后把不同前提当成同一请求。
关闭后的执行收口先确认撤权，清理执行不重新获得正文发布权限。

PR 保存固定的 base/head 提交；发布候选交接完整 commit OID，不重新解析可变
工作分支。平台在自己的 Git 对象库核验并保留候选引用。新实施工作树从已发布
候选接续。合并是显式的候选采纳动作，不自动验收或关闭关联 Issue；拒绝冲突、
base 移动及不安全的检出状态。合并前将 source 和其它 worktree 切离目标 base。

Git 引用与 SQLite 不是同一事务。合并意图持久保存，重复同一请求回读原回执；
若引用更新而记录未完成，重试核对实际引用后收敛，不重复应用代码。

## 从旧 Local 实例迁移

旧 `[local]` 将平台和 Harness 数据混在一起，新 binary 明确拒绝继续使用。
停止旧 daemon 和对象写入入口，保留现场并核实 prepared/unknown 写入、合并意图
及原生执行停止结果。声明独立平台后显式导入旧 runtime 数据库：

```shell
braid platform import --config /absolute/path/to/platform.toml --from-runtime-db /absolute/path/to/old/state/braid.sqlite3
```

导入保留原数据库及对象编号、local:*身份、事件、正文、评论、关联、候选与
对象命令回执。执行会话和运行回执留在 Harness；旧会话不自动取得新写授权。
将 Harness 配置改为 `[builtin]`，保留原 Profile workspace 和 runtime 路径，
应用 Harness 迁移后再接入平台。新入口重新投递稳定事件，已有 delivery 去重，
不以旧 consumed_at 最大值跳过未消费事件。不可继续用旧 binary 写原库，否则
会形成两份对象权威。

没有模型调用时，`serve --transport-only` 可实际核对上游接入、事件和队列，
不能证明 Agent 已完成产品工作。当前不提供 GitHub API 全兼容、双向同步、
远程多人权限系统或完整跨进程崩溃恢复。
