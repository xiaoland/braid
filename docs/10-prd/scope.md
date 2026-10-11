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
