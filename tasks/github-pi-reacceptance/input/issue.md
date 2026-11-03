# GitHub + Pi 原生插件验收：离线笔记 CLI

请维护本 Issue 中的接受设计，随后通过关联 PR 实现离线笔记 CLI。业务范围仅
`e2e-notes/20261011-github-pi-native-v2/`，不修改仓库其它文件、旧应用或历史运行。
没有业务基线答案；只使用 Python 标准库，不依赖外部网络服务。

`python3 notes.py --store <路径> add <标题> <正文>` 新增笔记并返回稳定唯一 ID；
`list` 按创建顺序显示未归档笔记；`archive <ID>` 归档且保留正文，重复操作不产生
新记录。跨进程仍可读取。标题和正文支持中文、空格、引号和换行。无效 ID、损坏
JSON 或错误根结构必须给具体错误和非零退出码，不能覆盖原数据。README 说明实际
命令与数据存储位置。

Issue Agent 只负责设计、实施请求与工作记忆，用 braid gh pr ensure 建立关联 Draft PR；
PR Agent 在专属工作树完成真实实现、Git 提交、push 和应用操作，自己更新 PR 工作记忆。
不要自行合并。禁止开发基础设施测试和模拟器；验证来自真实调用应用。

本轮使用 Pi 的 subagents 与 background 插件协作：至少一次同步子 Agent 形成可用
实现建议，至少一次异步子 Agent 完成实际实现或应用操作并取得结果；子 Agent 与根
角色全部只能使用 minimax/MiniMax-M3，不得改模型、自动fallback或使用宿主其它登录。
后台有限命令使用 PBB 并保留正确handle、原始退出状态及输出，在交付之前消费结果。
需要常驻服务时明确标为service，不能把有限工作伪装service；所有副作用由所属执行管理。
不要为演示重试制造假服务错误。请保持公开讨论简洁，不发表内部推理。

所有应用数据、材料、日志、临时文件和缓存只放当前 WorkSSD 工作目录或
`/Volumes/WorkSSD/Development/braid-gh-pi-reacceptance/`；不能写 `/tmp`、系统盘、
其它仓库或旧运行，也不靠 TMPDIR 代替实际路径核对。凭据只交 SDK，不能写入应用、
评论或证据文件。正常 GitHub 操作用当前 BRAID_CONFIG，不寻找旧实例。

可见标记：BRAID_VISIBLE_GH_PI_NATIVE_V2_20261011。
<!-- BRAID_HIDDEN_GH_PI_NATIVE_V2_20261011：此隐藏正文不应进入当前 Context。 -->
