## Purpose and Pressure

Long Coding Agent sessions accumulate tool output, superseded requirements, and
stale operational facts. Provider compaction helps with token pressure but does
not know which GitHub edits, hidden comments, resolved review threads, or
current metadata supersede that history. Traditional chat also leaves settled
design and implementation state scattered across a private transcript.

Braid makes the collaboration surface itself the compacted memory:

- an Issue description carries the current design;
- a PR body carries the current implementation intent and state;
- visible comments retain discussion and Agent messages;
- folded or resolved discussion keeps identity and lifecycle metadata but not
  body content;
- current GitHub metadata and relationships remain available without expanding
  every related object recursively.

An Agent may therefore replace a stale provider session without losing the
team's current understanding. Humans can correct that memory with ordinary
GitHub edits instead of manipulating a private Agent transcript.

在一项已明确激活、仍然有效的职责内，Braid 还负责让当前协作约定的相关变化
可靠到达负责的 Agent。更新记忆、持久接收待处理输入、Agent 实际接手是不同的
产品结果；不能因会话空闲、另一条读取路径先看见新版或入口故障而静默丢失输入。
Agent 可以讨论、修订、实施、澄清或明确等待，并不需要每次变化都修改代码。
Human 不应为了补偿内部投递问题再发一次 `@braid`。

快速连续变化可以合并，处理当前有效版本；Braid 不保证还原 GitHub 未被观察到的
每个中间版本，也不承诺每次编辑各开一轮。新任务或新角色仍需明确授权，关联本身
不激活另一个 Agent，正文仍是工作数据，Human 的审查、合并和关闭权不变。
