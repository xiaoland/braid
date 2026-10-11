## Scope and Exclusions

The MVP includes Codex app-server, macOS arm64, real GitHub, SQLite, a free
Cloudflare tunnel for acceptance, `braid gh` writes, packaged installation,
OpenTelemetry, and the Issue-to-PR workflow above. Linux x86_64 follows with
the same contract.

当前实现还提供 Pi adapter，使用实际支持 prompt 接收回执、get_state、get_entries、
agent_settled 和 abort 收口的原生协议。它不承诺任意 Pi 版本、扩展或脱离原生管理
的后台执行都能安全接续；缺失协议能力或停止依据时明确阻断。

The MVP does not promise Claude adapters, hosted ingress, deterministic
multi-Agent merge, automatic merge, universal GitHub CLI parity, automatic
semantic readiness/acceptance, or recovery of a provider transcript that the
provider itself has lost.

内建协作平台与 GitHub 处于同一生态位，是 Harness 的另一种上游对象权威。
它独立承载仓库、Issue、PR、正文、评论、指派、事件及真实 Git 候选，即使没有
启动 Harness 也可讨论和维护工作项。Harness 接入一个上游，复用调度、记忆投影
及原生执行；停止或更换执行不改变平台对象身份和历史。候选发布与手工合并
独立于模型终态或产品验收。不承诺 GitHub API 全兼容、双向同步、远程用户权限
系统或自动验收。

## Braid Web Console

Web Console 属于整个 Braid，接收 OTLP、保存并可视化观测证据，统一阅读和
控制平台及 Harness。GitHub 与内建平台使用同一个 Harness 运行控制入口；
内建平台可独立启动，也可由 Console 便捷装配。对象、执行和观测分别保留
权威，不把遥测缺失当作工作完成。Console 不依赖 Factory，不执行 Braid CLI
作为后端桥，也不承担实验/赛事调度。
