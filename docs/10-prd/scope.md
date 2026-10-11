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

内建协作空间是独立 Braid 的另一种对象权威，承载 Issue、评论、指派、正文、
生命周期和关联 PR。它复用现有调度、记忆投影及原生执行，允许没有 GitHub App
的本地 Git 仓库持续协作。PR 对应真实 Git 候选，候选发布与手工合并独立于模型
终态或产品验收。当前入口是 CLI；不承诺 GitHub HTTP API 全兼容、平台用户权限
系统、Web UI、GitHub/Local 双向同步或自动验收。

## 本地 Console

独立 Braid 提供本地 Web Console，负责接收 OTLP、保存并可视化观测证据，
并通过直接应用接口管理 Local 对象和启停 runtime。对象和执行结果保留自己的
权威，不把遥测缺失当作工作完成。Console 不依赖 Factory，不执行 Braid CLI
作为后端桥，也不承担实验/赛事调度。
