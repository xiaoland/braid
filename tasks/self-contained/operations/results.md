# 本地协作对象实际操作

2026-10-11，在 `operations/objects/` 的隔离 WorkSSD Git 仓库和 SQLite 空间上执行 Braid 实际 CLI，没有启动模型、Agent 生成或 GitHub 操作。这些记录是本次人工实际操作的回执，没有新增测试入口或操作脚本。

`braid init` 生成不含 GitHub App 的配置；迁移回执显示数据库从 0 升至 4。随后创建 Issue #1，追加评论及指向第一条评论的回复，修改正文、指派 default Profile、关闭和重开。PR #2 使用同一个编号序列，关联 Issue #1。`context` 显示修订后的正文、两条评论及回复关系，`diff` 返回真实 Git tree 的补丁。

PR 创建时 head 为 `d2eb00a171d4fa0c4f76e7376fd638c5ea01d9d1`，base 为 `5d7172e0d4f4e56b23aec8be6813a15677543477`。在另一个干净已提交 checkout 上执行 `publish 2` 后，候选变为 `c1eeeb3ea86489f76774ccfda3f7a8f621bb4953`。`merge 2` 通过 Git ref CAS 将 `refs/heads/main` 更新至双亲提交 `83b82a83722827792a191af56b9e55136f270fb9`；该提交的 README 同时包含初始 candidate 和后续 published 内容。再次执行 merge 返回相同提交与 `recovered=true`，没有再创建或采纳一个提交。

另建工作树从发布的 `c1eeeb3…` 开始，实际 HEAD 与发布候选一致，文件包含 published 内容。原 head 名称 candidate 保留；运行后端新建 PR generation 从明确候选 OID 出发，不把创建时旧分支作为已发布候选。

实际拒绝的操作包括：无活动 assignment/session/turn 的 Agent CLI 写入，错误 repository 身份复用同一数据库，跨对象回复关联，向已合并 PR 发出 reopen，以及向正在检出的 base 分支合并。真实 Git 冲突记录在 PR #5 的 merge 状态中，包含 `README.md` 路径及冻结 head/base；目标分支没有移动。`show 5` 可直接读取持久错误。

回执见 `objects/` 下的 JSON、Markdown、patch 和 error 文件。配置、Git 仓库与 SQLite 现场保留在 WorkSSD 并由任务目录忽略。实现通过编译和严格 Clippy；源码与迁移由主 Agent 统一提交。

没有模拟进程崩溃或构造 Agent 运行状态；prepared→Git CAS→applied 的崩溃接续依据仍需真实崩溃场景资格确认。也没有运行模型来验证 Agent 自己调用这些工具的完整任务。

## 持续运行与队列

主 Agent 使用另一份隔离 source 和本地配置，先在停服状态创建 Issue 和评论，再以
`serve --transport-only` 启动。实际 health 返回本地服务就绪、provider disabled；
local_events 1、2 全部确认，原 Store ledger 分别持有 noop/consumed 与 wake/pending，
对应 wake batch 已为 runnable。原始回执为 `runtime/first.json`。

正常 SIGINT 停服后追加指派和评论，source 切换到 operation-staging 分支，再次启动。
4 条 local_events 均确认，4 条 delivery 的 duplicate_count 全为 0；原评论、指派仍在，
同一 batch runnable、event_count=3。配置 default_branch 仍是 main，未由当前 checkout
重新推导。实际停止后没有留下本轮 daemon。原始回执为 `runtime/restart.json`。

再以原生执行开启、明确缺少 BRAID_OPERATION_KEY 的状态启动，同一公开 health 返回
HTTP 503、ready=false、provider=unavailable，保留完整的缺失环境变量原因。该操作
没有开始模型或原生 Agent turn。现场未配置 OTel Collector，观察到明确 HTTP export
network error；没有用此现场宣称 telemetry 或模型全链路验收。回执为
`runtime/provider-unavailable.json`。

三份旧迁移在本轮没有改变。操作文件被 Git 忽略，摘要与源码一起保存；所有构建、
缓存、数据库和临时文件均位于 WorkSSD。

最终 binary 的 transport-only 服务重新启动并由 SIGINT 正常退出（exit 0）。
旧 GitHub config.example.toml 通过公开 config check；Local 实例的 github probe
在连接之前明确拒绝，未借此访问 GitHub。最终 build、严格 Clippy、fmt 和 diff
空白检查通过，未运行测试。
