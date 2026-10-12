# MiniMax 双上游验收：离线笔记 CLI

请先澄清并维护本 Issue 中的设计，随后通过关联 PR 实现离线笔记 CLI。业务范围是本轮 `e2e-notes/20261011-minimax-peer-upstreams/`；不修改仓库其它文件或已有应用。需要保持实现不依赖网络服务或第三方 Python 库。

使用 `python3 notes.py --store <路径> add <标题> <正文>` 新增笔记并返回稳定唯一 ID；`list` 默认按创建顺序显示未归档笔记；`archive <ID>` 归档而不删除内容，重复归档不产生新记录。进程退出后数据仍可读取。标题和正文允许中文、空格与引号。无效 ID 或损坏存储应返回具体错误和非零退出码，不覆盖已有数据。README 应说明实际命令与数据存储位置。

Issue Agent 负责设计、实施请求及工作记忆；PR Agent 负责专属工作树中的业务实现、真实 Git 提交和应用实际操作证据。请保持讨论简洁，发布可供采用的事实，不能仅以“完成”或原生执行退出代替产物验证。

可见验收标记：BRAID_VISIBLE_MINIMAX_PEERS_20261011。
<!-- BRAID_HIDDEN_MINIMAX_PEERS_20261011：此隐藏正文不应进入 Agent 当前 Context。 -->
