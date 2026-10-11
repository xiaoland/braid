# 本地协作空间

Braid 的内建空间持有自己的 Issue、评论、负责人和关联 PR。它无需 GitHub App、
webhook 或外部协作平台；原生 Agent CLI、Git 与模型服务仍分别负责执行和代码。
一个实例只使用一个协作后端，已有 GitHub 配置继续有效。

## 创建与启动

先准备一个有命名分支的本地 Git checkout，以及实际安装的 Pi 或 Codex。创建
配置时明确模型与 API key 环境变量名称；命令不会读取或保存 key 值。

```shell
braid init --source /absolute/path/to/repository --config /absolute/path/to/space/config.toml --name demo --adapter pi --executable /absolute/path/to/pi --provider deepseek --model deepseek-chat --api-key-environment DEEPSEEK_API_KEY
braid config check --config /absolute/path/to/space/config.toml
braid migrate apply --config /absolute/path/to/space/config.toml
braid serve --config /absolute/path/to/space/config.toml
```

配置的 `[local]` 保存稳定空间名称、本地 source 路径和明确默认分支，不能同时配置 `[github]`。
默认分支不会随 source 当前检出的分支改变。
当前单个 `default` Profile 可以承担 Issue 和 PR，两者仍有独立工作树和会话。
Codex 需要在配置中的 provider home 登录；初始化不会复制个人凭据。多个实例
运行时须为它们配置不同 health 端口。

## 工作项与讨论

下例沿用 `BRAID_CONFIG`，也可以逐次提供 `--config`。Issue 与 PR 共享编号，
从实际命令回执取得编号，不从下例推断已有对象身份。

```shell
export BRAID_CONFIG=/absolute/path/to/space/config.toml
braid local issue --title '改进导出流程' --body '说明用户目标和允许范围'
braid local list
braid local show 1
braid local assign 1 --profile default
braid local comment 1 --body '请调查现有导出入口并维护当前方案'
braid local edit 1 --body-file /absolute/path/to/current-design.md
braid local context 1
```

Issue 指派先物化会话，评论唤醒工作；PR 指派会启动实施。没有 daemon 时，
对象修改和待投递事件仍持久保存，下一次 serve 接续投递。对象正文是当前工作
依据，评论是讨论数据，不能获得系统指令权限。

`unassign` 撤销当前负责人；`close` 关闭工作项，`reopen` 重开。它们复用现有
Group 生命周期，不能以对象状态推断代码已被采用或用户目标已经验收。

## PR 候选与采用

PR 从真实 Git head/base 分支建立，并可关联一个 Issue。代码提交之后要明确
发布候选，diff 与合并才能消费当前交付版本。源 checkout 和 Agent 工作树共用
Git 对象，操作不会 push 到远端。发布保存当前提交身份；新的实施工作树从该
候选接续，不把最初 head 分支仍指向旧提交当作最新成果。

```shell
braid local pr --title '实现导出流程' --head feature/export --base main --issue 1 --body '实现范围与待验项'
braid local assign 2 --profile default
# 在实施工作树提交代码后，从该工作树发布候选：
braid local publish 2
braid local diff 2
braid local merge 2
```

合并是显式的候选采纳动作，不自动验收或关闭关联 Issue。命令核对冻结的 head
和 base，保留具体冲突及失败原因；分支变化后须先形成新的明确候选。为避免
移动已检出分支却让工作目录保持旧内容，合并拒绝任何工作树正在检出的 base。
调用前把 source 切换到另一分支，并确认其它工作树也未检出目标 base。

合并意图持久保存。若 Git 引用已更新而数据库记录未完成，后续重试核对实际
引用后收敛记录；不因重试重复应用代码。停止 daemon 不删除对象、评论或候选。

## 操作反馈与边界

`braid status` 和本地 health 提供执行事实，具体失败保存完整原因，并按 Profile
配置写入工作项状态评论。结果未知或不能确认旧执行停止时，自动替换会话会被
阻断。没有模型调用时，可用 `serve --transport-only` 实际检查事件投递和队列，
这种操作不能证明 Agent 已完成产品工作。

Agent 子进程使用配置入口并按当前工作树、assignment、session、turn核对写入
来源。它用于防止正常执行串写或失效会话回写，不是针对恶意本机进程的安全
沙箱。当前没有 Web UI、GitHub API 仿真、平台用户权限系统或双向同步。
