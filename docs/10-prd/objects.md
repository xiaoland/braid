## Product Objects

### Work Items and Context

- A **GitHub Issue** becomes an Issue Work Item through an Issue Activation.
  A provisioned GitHub Agent App assignment is the preferred native signal;
  an ordinary GitHub App is not a standard assignable user, so the PoC also
  accepts the first Trusted Braid Mention on a dormant Issue. Native assignment
  creates a session and remains idle; mention fallback carries the comment as
  a Wake Event and starts the first turn after materialization.
- A **GitHub PR** becomes a PR Work Item through a trusted `@braid` PR comment
  or the ActivationIntent produced by `braid gh pr ensure`.
- **Issue Context** contains the repository-qualified Issue identity, current
  title and description, material metadata and relationships, and its comment
  lifecycle projection.
- **PR Context** first contains the current projections of every directly
  Associated Issue, then the PR's own minimal implementation context. It is
  rebuilt from current state before every PR turn; it is never a creation-time
  snapshot.
- Full Context is Markdown or plain text for the Agent, never a JSON protocol
  dump. Event user messages are short references; the Agent can use `gh` to
  inspect the changed object.

The exact projection is the [Context contract](../20-product-tdd/context.md).

### Agent Profiles and Groups

An Agent Profile is a versioned Braid configuration containing a provider,
model, reasoning setting, Profile User Instructions, cwd/workspace policy,
and supported provider resources. Tags declare whether it can serve `issue`, `pr`,
or both. The Profile `workspace` names a clean source checkout, never the
Agent's cwd: every Agent Group session runs in a dedicated generation-scoped
Braid worktree (the Issue's sole Development branch when unambiguous,
otherwise the default branch; the PR head for a PR Agent).

Pi Profile 可以明确选择独立技能和原生角色材料。根会话模型与推理仍由 Profile
决定，Pi 内部 subagent 角色不是可指派的 Braid Profile。材料安装、Profile 选择
与会话实际装载分别需要依据；材料变化不得在保持旧 Profile 身份的情况下恢复
原生历史。工具和 MCP 由已支持的原生安装能力提供，目前没有跨 provider 的
tools/MCP 或 sandbox/approval Profile 配置接口。Factory preset 与 variant 属于
消费方的装配，不属于 Braid 的 Profile 对象。

Braid adds its own versioned System Prompt when materializing a Provider
Session. It explains GitHub Working Memory, Braid and `braid gh`, concise public
comments, and the Issue- or PR-specific role. GitHub Context remains delimited
working data rather than a system instruction.

The architecture can represent multiple parallel Agents without primary or
sub-agent roles. MVP acceptance deliberately uses:

- one active Issue Agent per Issue Agent Group;
- one Implementation Agent per PR Agent Group;
- one dedicated generation-scoped worktree per Agent Group session.

Multi-Agent fan-out is not rejected, but cross-peer ordering, semantic merge,
arbitration, and convergence are outside the MVP correctness claim.

### 内建空间的对象

Local Issue 和 PR 共享递增编号，使用稳定空间名称标识；编号不是 GitHub ID。
正文、评论、负责人及关联由 Braid 持久保存。当前沿用一个 Issue Profile 与默认
PR Profile，拒绝指派到没有对应执行者的配置。Issue 指派物化会话，后续评论唤醒
设计工作；PR 指派启动实施。关闭与重开沿用当前逻辑会话生命周期，空间本身持续
存在。评论和描述是工作数据，不获得系统指令权限。
