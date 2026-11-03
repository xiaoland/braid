# 从 root 任务自主推进

`braid run` 在内建协作平台上执行一次以 root Issue 为范围的工作。平台是独立的 minimal GitHub；`serve` 持续响应协作事件，`run` 让 root Agent 组织拆分、分派和成果整合，并保存可判断的运行结果。Web Console 属于整个 Braid，两个入口使用同一套调度和原生执行。

先按[内建平台指南](local.md)配置真实 Git checkout、独立平台与 Harness。平台须已运行，Harness 配置须包含实际 provider、模型和角色。业务输入可以只有一个 description 文件，但这不替代仓库配置、模型凭据或运行授权。首版 `run` 接入 built-in，不接入外部 GitHub。

```shell
braid run --config /absolute/path/to/harness/config.toml --run-id feature-a \
  --description-file /absolute/path/to/requirements.md \
  --delivery-ref refs/heads/run/feature-a --max-concurrency 2
```

现有 root 必须是没有父 Issue 的顶层任务；已作为其它任务子项的 Issue 不能直接注册为 run root。已有 root 则把 `--description-file` 换成 `--root-issue 42`，二者不可同时提供。description 创建的 root 通过固定请求 ID 去重；`run-id` 绑定输入、交付 ref、并发和已解析配置。同一个 ID 接续不能换需求或配方。完成后再次调用只回读既有完成结果，不重新生成。未确认旧执行停止时禁止替代启动。

每次 run 有独立 Harness ledger、工作树和冻结配置，存于原 `runtime.root/runs/<run-id>/`。平台仍保存唯一的 Issue/PR 工作图；run 不接管 `serve` 的旧指派和无关对象。description 或现有 root、子项和关联 PR 通过公开平台 API 读取和修改，不直接操作平台数据库。配置和原生历史仍包含实例允许保存的材料，应按该实例的授权范围保留。

## 协作和交付

root 使用专属协调职责，Agent 自行决定拆分方式、依赖顺序和何时指派。Harness 不按需求编号拆工作。子 Issue 正文应保留必要需求、共享决定、依赖和交付 ref；root 等待成员时应结束当前轮次以让出 Issue 组的执行机会。PR Agent 实现并发布真实 Git 候选；业务技能和固定交付步骤由调用方配置，不自动植入 Factory 的 Web 应用、基础 PR 或最终整合 PR 配方。

子 Issue 用 `braid object issue --title ... --body-file ... --parent <父Issue>` 创建。创建和父子关系在同一平台事务提交，不提供事后随意重绑入口。创建未指派对象不启动成员。平台将子项生命周期、候选发布、讨论和设计变化送回祖先；讨论及设计通知仅包含对象和变化类型，正文仍按需读取。root 根据当前对象事实采用成果，不依赖模型会话记忆推断交接完成。

run 的交付 ref 必须明确写为 `refs/heads/...`。平台冻结 root、run ID、Harness client 与该 ref 的关系，Agent 不能修改这项运行授权，范围内 PR 的 base 和合并目标由平台校验。运行初始化缺失交付分支，但不隐含向默认分支合并。建议每 run 使用独立交付分支；合并仍检查确切候选、base 是否移动以及分支检出状态。

root writer 只能协调自己的后代 Issue 和关联 PR；子项不获得任意仓库管理身份。注册 root 关闭时整个范围 writer 立即失效，子项不能在收尾期间继续发布。普通 `serve` 的关联 Issue 关闭规则不因此改变。root 应在范围义务收敛后最后关闭；提前关闭不作为成功，需由操作者明确重开才能继续。

`--max-concurrency` 限制同时在途的 Braid 轮次，root 也计入，默认 2。它不限制空闲 Pi 会话、Pi 内部 subagents 或 background 服务的数量；当前 Issue/PR 两组执行还限制各自组内同时在途轮次。保留物理会话可能增加内存占用，不能把这个参数当作总进程或宿主内存上限。当前不自动驱逐健康空闲会话来释放容量。

## 结果和停止

运行输出结构化结果，保留 run ID、root、状态、交付 commit、具体原因与停止确认。状态含义如下：

| 状态 | 含义 |
| --- | --- |
| `completed` | root 声明完成，范围义务收敛，确定交付 commit 对应已发布或合并的范围候选，未决写入及执行收尾满足合同。 |
| `blocked` | 明确前提或义务未满足，例如 root 提前关闭、合并冲突或执行无法恢复；保留具体原因。 |
| `failed` | 运行发生具体设施错误。 |
| `stopped` | 收到主动停止，不等于需求完成。 |

root 开放且暂时没有可执行工作时继续等待，不因安静自动成功或失败。CLI 仅 completed 正常成功退出；其余状态保留结果并以非零结束。业务满足需求仍须对应用实际验收；completed 只说明团队交付声明与运行闭环已完成。

终态与 `stopConfirmed` 分别读取。实际原生停止不确定时不允许新运行覆盖。CLI 使用正常终止信号请求停止；Console 的停止按钮调用同一监督器。关闭 Harness 后平台对象和观测历史保留，不自动删除应用、候选或原生证据。异常退出不能推断为安全接续；核对原件并处理旧执行所有权后才继续。

Console 的运行方式选择 `run` 后可输入 description 或 root 编号、run ID、独立交付 ref 和并发轮次。“填入最近运行参数”只预填原始输入，不自动启动；从 description 创建的 run 接续仍保留原 description，不能借同一 ID 换为生成后的 root 编号。控制经同一个内进程应用接口，不启动 CLI 子进程；Harness snapshot 指向当前 run ledger，OTLP 只提供观测，不决定完成。更多命令回执和停止边界见[Console 指南](console.md)。
