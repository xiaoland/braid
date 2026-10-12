# Braid 的 Pi 原生材料

本目录提供独立 Braid 的锁定 Pi 1.1.0 材料。插件按安装时的明确选择安装和启用，
不读取系统全局 Pi 扩展；CLI 和 RPC 使用同一入口。共同生命周期与停止合同归
[provider 技术说明](../../docs/20-product-tdd/app-server.md)。

`pi.mjs` 准备隔离环境与显式材料；`sdk-host.mjs` 使用 Pi 根包公开导出的 SDK
构造会话、资源和工具，复用公开的 RPC/print 实现；`rpc-adapter.mjs` 增加 Braid
停止、服务交接和实际运行身份。Braid 不再修改 Pi CLI/RPC bundle、会话生命周期、
资源加载器和 Bash 实现来注入这些策略。
受管入口支持 Braid 使用的 RPC，以及子 Agent 使用的 JSON/text print、文本任务
文件和图片附件。它不是完整交互式 Pi CLI；未实现的原生 CLI 选项明确报错，
不能忽略选项后开始另一种模型操作。

在有 Node、npm、curl 与 tar 的开发宿主安装到新的绝对目录。不要覆盖正在使用的安装，
保留旧版本和执行回执。Mac 本项目的目录必须位于 WorkSSD。

```sh
node native/pi/install.mjs /Volumes/WorkSSD/Development/braid-runtime/pi-1.1.0 \
  --plugins subagents,background
/Volumes/WorkSSD/Development/braid-runtime/pi-1.1.0/pi --version
```

安装使用同一份锁定依赖图，以 npm workspace 选择基础 SDK 和指定插件，
仅应用基础补丁与所选插件补丁。未选择的插件不安装、不修改；插件之间不隐含绑定，
例如 background 可单独选择。安装下载 Node 官方 24.15.0 对应平台发行包，
核对内置 archive SHA-256 后交付独立的 `bin/node`、许可证、`pi` 入口和
`runtime-manifest.json`。支持 macOS/Linux 的 arm64 与 x64；manifest 保留发行身份
和实际文件哈希。安装器不复制可能依赖 Homebrew 动态库的宿主 Node。
安装目录中的缓存和临时目录也位于该目录；安装时会实际启动交付的 Node，
Pi 的实际能力仍须通过 doctor 与运行核实。
可选插件名称为 subagents、background、context7、fff、mcporter；默认不启用插件。
FFF 使用平台原生依赖，安装失败应保留原始错误，不降级为不可用的工具。

基础发行保留两类小范围上游补丁：`compaction.thresholdTokens` 提供提前压缩触发点，
计算为它与真实模型容量减 reserve 的较小值，不伪造模型容量或改变总结预算；
pi-ai 的连接重置错误分类保留已观察到的 `connection reset by peer` 重试行为。
对应的公开设置或错误分类被上游提供并取得实际反馈后，再退役这些补丁。
subagents、background、Context7 和 MCPorter 的局部补丁仅随相应插件安装，
仍包含尚无公开接线点的插件内部修复及 owned process/service 登记。
它们不是无补丁上游发行，不应删掉停止或交接能力来换取零补丁。

锁定的 pi-subagents 0.56.0 对自定义角色 frontmatter 使用字符串列表解析，
`fallbackModels: []` 和 `fallbackModels: false` 都会成为模型名称，不能用于禁用回退。
只有主模型时省略该字段；需要限制模型范围时配置插件的 modelScope，并通过实际
角色读取核对候选，随后从根与子会话的调用记录确认真正使用的模型。

将实例的 `[[runtimes]]` 中 `executable` 设为安装的 `pi`，
`native_runtime` 设为同目录的 `runtime-manifest.json`。`braid setup` 对明确指定
的原生入口识别相邻 manifest；`braid doctor` 核对材料身份，实际会话启动再核对
运行能力。缺失或漂移不能通过系统全局 Pi 回退隐藏。

受控 Bash 工具与 background 插件使用非登录 shell，并从原生入口移除 `BASH_ENV` 和 `ENV`。
工具消费实例提供的 PATH、HOME、临时及缓存目录，不依赖宿主登录 profile 注入 alias、函数或路径；
所需工具须在运行配方中明确准备。模型显式执行的命令仍保持原意，包括主动调用登录 shell 或 source 脚本。

选择 MCPorter 时，执行目录的 Unix socket 路径必须满足系统长度限制。路径过长时
明确设置 `BRAID_NATIVE_SOCKET_ROOT` 为同文件系统上足够短的私有绝对目录，
例如 WorkSSD 上的短目录；不能默认改写到 `/tmp`。未选择 MCPorter 时不要求此项。

Braid 为每个物理执行分配独立目录；关闭后的记录仍保留。人工处置遗留执行前，
先确认对应的安装、execution ID 和目录，再用该安装的固定 Node 调用：

```sh
<install>/bin/node <install>/managed.mjs cleanup \
  --execution-dir <execution-directory> --execution-id <execution-id>
```

回执中的 unknown 表示不能证明归属或停止；父进程退出不能替代后代清理证明。
操作不能撤销已经发出的网络请求，也不是同账户进程的安全沙箱。较晚 cleanup 成功只证明
回执时点的执行收口，不能改写此前 deadline 内停止未知的运行结果。
