# Braid 的 Pi 原生材料

本目录提供独立 Braid 的锁定 Pi 1.0.2 材料。插件按安装时的明确选择启用，
不读取系统全局 Pi 扩展；CLI 和 RPC 使用同一入口。共同生命周期与停止合同归
[provider 技术说明](../../docs/20-product-tdd/app-server.md)。

在有 Node、npm、curl 与 tar 的开发宿主安装到新的绝对目录。不要覆盖正在使用的安装，
保留旧版本和执行回执。Mac 本项目的目录必须位于 WorkSSD。

```sh
node native/pi/install.mjs /Volumes/WorkSSD/Development/braid-runtime/pi-1.0.2 \
  --plugins subagents,background
/Volumes/WorkSSD/Development/braid-runtime/pi-1.0.2/pi --version
```

安装使用锁定依赖并应用有序补丁，下载 Node 官方 24.15.0 对应平台发行包，
核对内置 archive SHA-256 后交付独立的 `bin/node`、许可证、`pi` 入口和
`runtime-manifest.json`。支持 macOS/Linux 的 arm64 与 x64；manifest 保留发行身份
和实际文件哈希。安装器不复制可能依赖 Homebrew 动态库的宿主 Node。
安装目录中的缓存和临时目录也位于该目录；安装时会实际启动交付的 Node，
Pi 的实际能力仍须通过 doctor 与运行核实。
可选插件名称为 subagents、background、context7、fff、mcporter；默认不启用插件。
FFF 使用平台原生依赖，安装失败应保留原始错误，不降级为不可用的工具。

将实例的 `[[runtimes]]` 中 `executable` 设为安装的 `pi`，
`native_runtime` 设为同目录的 `runtime-manifest.json`。`braid setup` 对明确指定
的原生入口识别相邻 manifest；`braid doctor` 核对材料身份，实际会话启动再核对
运行能力。缺失或漂移不能通过系统全局 Pi 回退隐藏。

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
操作不能撤销已经发出的网络请求，也不是同账户进程的安全沙箱。
