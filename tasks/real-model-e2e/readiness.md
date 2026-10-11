本文件保留执行前准备时的事实；本轮已结束，最终结果见 [results.md](results.md)。

# 真实模型与双上游准备回执

本回执只记录准备事实，不授予模型调用、GitHub 写入、隧道启动或数据库迁移授权。凭据值没有读取输出；没有启动模型、Pi turn、Braid runtime、Collector 或 tunnel。

## 已准备的隔离材料

- GitHub 目标仓库为私有 `xiaoland/braid-poc-test`，当前只读克隆到 `/Volumes/WorkSSD/Development/braid-e2e/source/github`；初始 `HEAD` 为 `6425f01e20ed4894b38ef4adc5da359896d07c20`，工作树干净。
- 内建上游使用同一初始提交的独立克隆 `/Volumes/WorkSSD/Development/braid-e2e/source/builtin`。两个克隆使用独立 Git 对象库，没有修改 GitHub 远端或旧的 `~/.braid` checkout。
- 两份 Harness 配置位于 `config/github.toml` 与 `config/builtin.toml`。运行数据库、备份、worktree、Pi home、服务端口和 OTLP service name 都不同，均在 `/Volumes/WorkSSD/Development/braid-e2e`。
- 内建平台配置位于 `config/builtin-platform.toml`，计划监听 `127.0.0.1:18282`；平台数据库和备份路径独立于 Harness。尚未执行 `platform init`，因此没有创建平台数据库或对象。
- 两个 Pi home 都有独立的 `models.json`，内容只登记官方 MiniMax M3：provider `minimax`、实际 model/wire id `MiniMax-M3`、`https://api.minimax.cn/v1`，API key 只引用 `$MINIMAX_API_KEY`。用户选择名仍是 `minimax-m3`，但 Pi 会把 model id 原样送入请求，因此配置已统一为官方大小写 wire id，避免 alias 造成 404。

## 配置与执行器事实

候选 `/Volumes/WorkSSD/Development/braid-e2e/bin/braid`（0.3.2，SHA-256 `50719bcd2adc3e2a261f7a703bf9b572bb0fb2f7db0584edbcbacec6ca081d6f`，源码 `1fdef82`）对两份 Harness 配置的 `config check` 均通过，结果为 schema 3，profiles 为 `issue-minimax` 与 `pr-minimax`，trace sample ratio 为 1。GitHub 配置引用已有 App ID `4780313`、已有私钥和 webhook secret 的绝对路径，但没有复制或改写旧系统盘实例；内建配置没有 GitHub 凭据。

本机 Pi 为 `1.0.2`，Braid 的 Pi adapter 会把配置中的 `provider`/`model` 转成 `pi --provider minimax --model MiniMax-M3`，把独立 home 作为 `PI_CODING_AGENT_DIR`，并把 `MINIMAX_API_KEY` 传给子进程。Pi 的 OpenAI-compatible transport 将 `model.id` 原样提交，因此没有隐含 alias 映射；`models.json` 结构沿用当前 Factory/Pi 的 `providers` → `baseUrl`/`apiKey`/`models` 契约。没有运行 Pi help 以外的模型操作；当前全局 Pi catalog 没有本轮官方 `/v1` 接线，所以真实运行必须使用这两个独立 home 的 models.json。

MiniMax 凭据来源明确为 `/Volumes/WorkSSD/Development/agentic26/.env`。只读解析确认 `MINIMAX_API_KEY` 非空，`MINIMAX_BASE_URL=https://api.minimax.cn/v1`；启动加载器 `load-minimax-env.sh` 只提取这两个名字，不 source 整个 `.env`，也不打印值。官方 M3 的现有计费记录是普通按量，≤512K 输入时输入/输出/缓存读取为 `2.10/8.40/0.42 元/M`，账户余额和套餐额度仍未知。

## GitHub App 与隧道边界

此前只读 `braid doctor` 已验证 App 私钥、webhook secret、App 身份和 installation 可访问 `xiaoland/braid-poc-test`；当前仓库 GitHub hooks 数量为 0。使用现有 App 私钥生成的短生命周期 JWT 只读 GET 已确认：可见 installation 仅有 `157947940`（账号 `xiaoland`，selected repositories，权限含 issues/contents/pull requests 写、metadata 读）；App-wide hook config 的 URL、JSON content type 和 `insecure_ssl=0` 存在，secret 存在。URL 与 secret 均未写入或输出。当前 `gh` 用户 token 仍只作目标仓库只读核对，不代表 App-wide 配置。

Braid 的 `serve --tunnel` 使用 Wrangler Quick Tunnel，并会调用 App-wide webhook 配置；它不是单仓库 hook。故本轮不能直接启动 `--tunnel`：这可能影响同一 App 的其它 installation 或正在运行的 Harness。内建模式明确禁止 tunnel。优先方案是先取得 App-wide installation/webhook 清单和影响范围，再决定是否使用一次性 Wrangler tunnel；若不接受 App-wide 影响，就需要一个已固定、已授权且指向本次独立 ingress 的外部 webhook 入口。

本机没有 `cloudflared` 或 `ngrok`。旧 `/Users/lanzhijiang/Library/pnpm/bin/wrangler` 实际执行报 Node `MODULE_NOT_FOUND`；已在 WorkSSD 隔离安装 Wrangler `4.149.0`，入口为 `/Volumes/WorkSSD/Development/braid-e2e/wrangler/node_modules/wrangler/bin/wrangler.js`，`tunnel --help` 已确认 `tunnel quick-start <url>` 命令契约。未启动 tunnel，pnpm home、store、依赖均在 WorkSSD；两份配置已改用该绝对入口。

## 仍未就绪的前提

- `MINIMAX_API_KEY` 已存在于指定 agentic26 env，但没有执行真实请求；模型费用、工具调用和流式行为仍待明确授权后取得真实证据。
- Collector 计划使用 `127.0.0.1:14318`，当前没有启动；两个 Harness 配置已指向该独立 endpoint。
- 内建平台尚未初始化，不能启动 builtin Harness；初始化和对象种子属于下一阶段实际验收输入。
- GitHub 旧配置的 Codex/schema pin、旧数据库和旧系统盘 runtime 均未复用。真实验收应使用当前 Braid binary、这些新配置和新数据库，并在启动前单独决定 migration。
- 当前准备没有改任何 GitHub 对象、App webhook、旧实例、项目源码或 Factory 源码。
- App JWT 可见 installation 目前只有目标账号这一条，但 App-wide webhook 仍是共享边界；在实际运行前仍需把 webhook 变更与恢复纳入显式验收窗口。目标仓库的 open Issue/PR、历史对象和活动进程核对见本 packet 的追加证据；不得让新 ledger 自动接管旧对象。

## 目标仓库与活动隔离核对（只读）

- `xiaoland/braid-poc-test` 当前 open Issue 为 0，open PR 为 0；历史 Issue 为 36 个且全部 closed、无 assignee；历史 PR 为 #24 和 #2，均 closed、无 assignee。
- App installation `157947940` 的只读 installation-token 查询显示共有 2 个 selected repositories：公开 `xiaoland/braid` 与私有 `xiaoland/braid-poc-test`，均默认分支 `main`。因此即使只有一个 installation，App-wide webhook 仍覆盖至少这两个仓库，不能把隧道影响范围缩小为验收仓库。
- 历史 Issue comment 中出现 `braid-of-xiaoland[bot]` 的 23 条记录，涉及 Issue #1、#2、#11、#12、#14，最新时间为 `2026-09-03T04:36:13Z`。这些是历史对象，不能让新 ledger 自动重认或继续驱动。
- 只读进程核对未发现候选 Braid、旧 Braid、Pi、Wrangler 或 cloudflared 活动进程；没有跨过程 Harness 需要停用。

Wrangler help 核验当时只设置了 WorkSSD 的 `PNPM_HOME`，没有启动或下载 cloudflared。实际启动方案必须在外层显式设置 `TMPDIR=/Volumes/WorkSSD/Development/braid-e2e/wrangler/tmp`、`XDG_CONFIG_HOME=/Volumes/WorkSSD/Development/braid-e2e/wrangler/config`、`XDG_CACHE_HOME=/Volumes/WorkSSD/Development/braid-e2e/wrangler/cache` 和 `WRANGLER_LOG_PATH=/Volumes/WorkSSD/Development/braid-e2e/wrangler/logs`；Wrangler 4.149.0 的 cloudflared 缓存位于其 global config 下，因此这些目录可保证下载、缓存、日志和临时文件不落系统盘。源码未发现 `WRANGLER_HOME` 作为有效控制变量；若预置 cloudflared，则另设 `CLOUDFLARED_PATH` 指向 WorkSSD 文件。上述目录仅是启动时接线要求，本轮未创建或运行 tunnel。
