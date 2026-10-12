# Braid Web Console

Console 与 Braid 一起交付，不需要 Python、Node 或前端构建环境。它接收 OTLP
HTTP/protobuf，保存原始批次和解码记录，并在浏览器中展示 traces、logs、metrics
和 provider 证据。Console 属于整个 Braid，接入平台对象 API 与统一 Harness 控制，
不属于内建平台的专属运行模式。
HTTP 后端调用 Braid 的应用模块，不执行 Braid CLI 子进程。

先按[内建平台指南](local.md)分别声明平台与 Harness，再由 Console 便捷装配：

```shell
braid migrate apply --config /absolute/space/config.toml
braid console --platform-config /absolute/platform.toml --config /absolute/space/config.toml \
  --database /absolute/space/observations.sqlite3 --listen 127.0.0.1:4318
```

遥测数据库必须与 runtime 数据库分开。Console 仅监听 loopback；浏览器打开输出
的地址。平台库、Harness 库、遥测库必须各自独立。Mac 本项目的配置、数据库
和临时数据都应位于 WorkSSD。`--config` 可为 GitHub 或内建上游；只有
`--platform-config` 时也可管理对象。两者均省略时只提供 Collector 和观测阅读。
已经独立启动平台时，不再提供 `--platform-config`，Console 按 Harness 的
`builtin.endpoint` 接入其 API。

Console 启动后 runtime 尚未启动。页面中的“启动”操作携带当前 generation；
“仅传输”处理本地对象和队列，不启动模型。普通启动会使用配置中的真实 provider，
需要对应凭据和运行授权。托管 runtime 在内存中使用本 Console 地址作为 OTLP
endpoint，不改磁盘配置。停止 runtime 后页面、Collector 和对象操作仍可使用，
停服期间平台继续接受对象操作，稳定事件等待 Harness 从自己的 cursor 接续。
平台服务是否可用、Harness 是否执行与最后观测时间分别展示；启动平台不启动 Agent。

## 阅读证据与当前状态

概览将 Collector 收到的数据、新鲜度和 runtime 权威状态分别展示。trace 时间轴
可以查看 Span、事件和缺失父节点；日志和 provider 原始事件可按 instance、session、
turn 或 trace 阅读；metrics 保留实际数据点和分布。原始批次可下载，接收时间与
事件发生时间各自保留。采样、未关联记录和接收错误会造成缺口；没有新数据不能
证明 Agent 空闲，模型 token 增长也不能证明任务进展。

Collector 接收 `/v1/traces`、`/v1/logs`、`/v1/metrics` 的
`application/x-protobuf`，支持 identity 和 gzip 编码。原始请求与解码记录原子保存
后才返回成功。分页查询使用稳定递增记录游标；重发批次保留接收事实与重复标记，
不能把 metric 的累计数据点相加成额外消耗。HTTP 错误保留具体状态和原因。

完整 provider 证据仍服从 runtime 的采样设置。它通过明确 session 与 turn 身份
关联，不从正文或相近时间猜归属。未关联的连接输出保留为连接级证据。SQLite 与
provider 决定当前对象和执行事实，OTLP 只提供观测历史，不承担可靠命令投递。

## 操作与回执

内建平台对象页可创建 Issue/PR、编辑正文、评论及回复、指派或解除、关闭或重开、
关联 PR、发布候选和合并。修改携带开始编辑时的 revision；冲突保留草稿，用户
查看最新正文后再决定是否重新提交。发布可以明确选择配置 source 的当前已提交 checkout，或该 PR 当前责任登记的
worktree。Harness 核验责任后交接固定提交 OID，平台在绑定 Git 对象库中核验并
保留候选；浏览器不能提交任意路径。Git 合并沿用平台对象的持久意图和 base 引用检查，
不是 Git 与 SQLite 的跨系统原子事务。

Diff 显示生成该内容时的对象编号、revision 及固定 head/base OID。它与详情页
版本不一致时，页面提示详情已过期并提供刷新；该快照不会自动改变合并使用的
revision 前提，采纳前应重新核对当前候选。

每次命令带 request ID。同一个 ID 不可换成另一份命令，响应丢失后查询原回执，
不自动重新执行修改。对象回执表示对象与事件已接受，不能证明 Agent 已阅读或
业务完成。runtime 启停回执也只表示请求接受；只有监督器收到全部 worker 的
真实停止结果才展示“已停止”。停止失败展示具体错误并禁止替代启动。

若 Console 异常退出而没有保存停止确认，下次使用同一实例会展示阻塞状态，
不会把旧执行当作已经结束。需要核对并处置该实例原生执行后再恢复；当前版本不
提供网页“强制清除”来绕过停止证明。Console 可与托管平台和 Harness 处于同一进程，尚不提供
进程级故障隔离、暂停/恢复、远程访问、多人权限或外部 GitHub 对象修改入口。GitHub 接入拥有同一 Harness 启停入口，
对象编辑仍使用该上游已有操作方式；不能用内建平台 UI 向 GitHub 伪装对象写入。
外部隧道接入 managed GitHub ingress 时，App webhook URL 必须包含 `/webhook`
路由；仅公网 host 根路径不会接收对象事件。以 GitHub 的实际 delivery 响应
确认投递，不把隧道存活或另一路径的签名请求当作当前 App URL 已生效。

## 数据边界

遥测可能包含完整正文、工具输出、路径和配置所允许的敏感诊断。采样控制容量，
不是保密措施；Collector 数据及原始批次只应交给该实例授权操作者。Console 没有
自动删除历史数据，需由操作者停止服务后管理数据库备份与保留范围。

页面以文本呈现不可信内容，不执行记录中的 HTML。HTTP 接口校验 loopback Host
和写操作 Origin，浏览器不能提供任意 shell 命令、PID 或文件根目录。这些措施
防止网页误驱动本机接口，不把拥有本机账户权限的进程视为隔离的对手。

## 选择常驻服务或 root 运行

配置 built-in 上游后，运行方式可以选择常驻 `serve` 或有限 `run`。run 输入为已有
root 编号或 description（二者择一），并声明 run ID、交付 ref 和最大并发 Braid 轮次。
首版 GitHub 上游仍使用 serve；run 不能与“仅传输”同时启用。

`POST /api/runtime/commands` 保持原 request_id、operation、expected_generation 合同，
启动 run 时附 `run_request`，其字段为 runId、rootIssue 或 description、deliveryRef 和
maxConcurrency。状态与回执分别展示 run 结果和 stop_confirmed；确认停止后的 blocked/failed
结果可以由明确新启动接续，不确定停止仍禁止覆盖。run 的确定交付 commit 不等于应用验收通过。
`/api/harness/snapshot` 阅读当前 run 的真实 ledger，平台对象和 OTLP 保持各自权威。
完整输入、配置冻结、交付和接续方法见[自主运行指南](run.md)。
