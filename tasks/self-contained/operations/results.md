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

## Web Console 与可观测性反哺（2026-10-11）

独立 Braid 增加 Console 的 OTLP 接收/证据数据库/查询/可视化与直接控制接口，
未迁入 Factory 的实验或赛事生命周期。所有新现场在 WorkSSD；没有模型、外部
GitHub、测试或额外自检运行。当前产品和操作合同见 docs/user-manual/console.md、
docs/20-product-tdd/README.md 与 docs/40-deployment/README.md。

实际 SDK 三信号已通过公开诊断命令发送，Collector 保留完整原始批次、属性、
资源身份和纳秒发生时间。原样和 gzip 重发保留各自接收事实与重复标记；gzip
原件下载 687 字节 SHA-256 与发送原件一致，不被浏览器 HTTP 自动解压。游标
分页、trace 过滤、正常重启保留旧批次均实际确认。最终版本的坏 protobuf 400、
错误方法 405、跨 Origin 403 和真实 SQLite 写锁 503 均为 protobuf Status，保留
具体错误及 SQLite code 5。Web 查询错误仍为 JSON。先前 500/JSON 的回执保留为
修复前事实，不能作为最终行为。超限 413 明确说明正文未捕获。原件在 collector/。

浏览器实际完成 Issue #1 创建、评论、编辑、指派 default、关闭和重开；评论中的
script 以文本显示。双页面旧 revision 6 与新增评论后的 revision 7 冲突，HTTP 409
返回具体实际/期望 revision，旧草稿保留。导航取消也保留草稿，操作中阻止切换。
独立 HTTP 操作确认重复 request ID 不重复创建、不同 payload 复用 ID 返回 409、
两个同 revision 编辑只有一个被接受；正常进程重启后持久回执不变。对象 raw
回执在 objects/http-*，浏览器回执与截图在 console/。

浏览器真实建立 PR #2 并关联 Issue #1，读取 README 的实际 diff，发布配置 source
当前已提交候选，再通过 DOM 确认层合并。持久意图与 Git 一致：

- base：`700afd192d26fc8bcd3223892db9819c1e28b1e5`。
- head：`ae2e3dd18694621cd52f829dfa095fecc99744b2`。
- merge：`77288eed6e7d495130927d476a1a2c0a06f57915`，有上述两个父提交及候选正文。

同请求回执查询仍返回相同合并提交。控制候选只接受固定 source 或该 PR 当前
责任登记的 worktree；浏览器无任意路径或 shell 入口。并发 canonical merge
意图/CAS 防护已实现，未做强制崩溃资格验证。

Console 的仅传输 runtime 经浏览器启动、停止并重启，provider 始终 disabled。
DOM 停止请求先返回 accepted/stopping，再查询取得 stopped、无错误，Collector
保持在线。最终实际操作 generation 为
`01a1299c-9a6c-7980-ac8a-3221d3b49b3c`；停止前后接收真实 runtime/control/ingest/ACK
日志与 runtime transition metric。发生时间为 0 的日志使用 observedTime 展示，
不会显示 Unix epoch。代次指标最后改为 generation_started/generation_ended，
明确观察代次结束不证明原生执行已停止；具体停止证明来自 supervisor。

复制前轮真实 schema 4 数据库后，通过公开 migrate apply 升至 schema 5，并取得
迁移前备份。前后 local_items=1、local_comments=2、local_events=4、deliveries=4、
events=4、wake_batch_events=3，历史对象和事件数量保持；原现场未修改，见
console/upgrade/upgrade-observed.json。原迁移 1–4 没有改动。

严格 Clippy、编译、格式与最终嵌入资源浏览器回读通过。已合并 PR 显示候选操作
结束，移除来源、发布和合并控件，保留真实 Diff 与持久合并记录。最终隔离 Console
由 SIGINT 正常退出（exit 0），本轮服务全部关闭。没有证明 SDK
自动重试、真实模型 provider/session/turn 全链路或 Console 崩溃后的完整恢复。
Collector 与 managed runtime 同进程，不提供进程级故障隔离；未确认停止会阻断
替代启动。采样证据不能替代 SQLite/provider 权威或产品完成。
