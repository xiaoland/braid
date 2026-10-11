# Braid 自包含

2026-10-11，本轮实现和隔离实际操作完成，等待用户采用反馈。

用户授权：“你可以自由提交。接下来我们使得braid变成自包含的。”本任务面向独立 Braid，允许实现与本地提交；不据此发布、push、修改外部 GitHub 或运行付费模型。前轮缺陷修复已提交 `9ba295a`。

用户已纠正解释：“不要考虑factory消费braid；注意我们做的是‘反哺’；自包含是说，之前braid上游接入GitHub，现在它可以有一套自己的类github实现。”据此实现内建协作空间，不扩张安装器或复制 Factory 运行配方。

独立仓当前有内置 Issue/PR 提示、GitHub 对象与事件生命周期、配置生成、数据库迁移和监督运行；setup 只发现外部 Pi/Codex 并要求先安装。Factory 另行装配技能、扩展和模型配方，这些内容的产品归属需要按用途判断，不能整树复制。

一个实例恰好选择 GitHub 或 Local 对象权威。Local SQLite 持有协作正文，GitHub SQLite 仍是机械缓存，双方不双写或自动同步。共享现有 Context、queue、Group、provider 及停止恢复合同。Local 对象变化与待投递事件原子提交，桥接现有 ingest 的重试以固定事件身份去重。空间持续运行，不设全对象完成退出或赛事交付 ref。

主 Agent 负责配置、迁移注册、公开命令路由、文档与整合；`local_objects` 负责 Local 对象存储、操作 CLI、候选合并；`collaboration_runtime` 负责后端边界、Group、本地工作树及持续运行；`native_local_env` 负责原生子进程 CLI 环境。`self_contained_boundary` advisor 的一实例一权威、保留后端边界与配置方案已采用。所有 Mac 产物、缓存与临时文件位于 WorkSSD。不编写或运行测试，以编译、格式、Clippy 和不调用模型的实际操作取得反馈。

当前在实施：互斥配置、schema 4 向前迁移、Issue/评论/正文/指派/关闭重开、关联 PR、真实本地 Git 候选与手工合并、CLI 查询及持续 serve。沿用当前单 Issue Profile 与默认 PR Profile，指派其它 Profile 明确拒绝。Agent CLI 依据专属工作树、有效 assignment/session/turn 核对来源；不是恶意本机进程的安全沙箱。首轮以 CLI 为操作入口，未扩展 Web UI。

完成依据：不提供 GitHub App 或凭据，用公开 CLI 实际完成创建、指派、讨论、关联 PR、候选采纳、daemon 重启后继续读写，核对事件与 Git 引用；明确区分无模型操作与完整 Agent 产品验收。真实模型或外部 GitHub 操作不由本次源码授权推导。


## 本轮结果

内建对象权威、互斥配置与初始化、持续运行、原生 CLI 环境和候选接续均已落盘。
PR 新代从明确发布的 OID 取代码，不依赖创建时旧分支；固定默认分支不随 source
checkout 改变。对象 Context 同事务读取，状态写回按 intent 幂等；实际执行不可用
时 HTTP 503 保留具体原因，对象 worker 失败向 serve 返回完整错误链。

Rust 1.93 最终 build、严格 Clippy、格式与 diff 空白检查通过。真实公开 CLI 完成
Issue/讨论/回复/正文/指派/关闭重开、关联 PR、候选发布、diff、双亲合并、重复
merge 收敛及冲突保留。主 Agent 实际核对停服输入、重启投递、runnable queue、
provider 不可用时就绪为 false，以及新 binary 正常停止。没有遗留本轮 daemon。
旧 GitHub 配置通过公开 config check，本地实例拒绝 GitHub 专属命令。
详细事实与边界见 [操作结果](operations/results.md)，原始回执和现场保留并忽略。

所有源码由主 Agent 统一本地提交，未 push、发布或部署既有实例。schema 4 必须
迁移后使用；没有运行模型、GitHub 全链路或崩溃注入，不将对象操作和编译视为
完整 Agent 产品验收。Web UI、多 Profile fan-out、评论删除/折叠与 GitHub 双向
同步未纳入本轮。前两项执行角色仍是既有单 Issue 与默认 PR Profile。
