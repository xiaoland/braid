# GitHub 网络与 App webhook 验收记录

本文件记录 GitHub 阶段的网络设施证据。当前只完成基线保存和启动封装，没有启动 Wrangler、没有修改 App webhook，也没有向 GitHub 写入对象。

## 基线

- 目标 ingress：`http://127.0.0.1:18180`；健康检查：`http://127.0.0.1:18181`。
- App installation：`157947940`，selected repositories 为 `xiaoland/braid` 和 `xiaoland/braid-poc-test`。
- App-wide hook 原始响应已保存至 `/Volumes/WorkSSD/Development/braid-e2e/github/network/app-hook-baseline.json`，权限 `0600`。只记录存在性：URL 存在、secret 存在、`content_type=json`、`insecure_ssl=0`；文件内容不得复制到报告或命令输出。
- installation 与 hook 查询均使用临时 App JWT；JWT、installation token 和响应临时文件均未保留。
- GitHub GET 返回的 `secret` 字段确认是掩码 `********`，不是可用于恢复的原 secret。恢复时不得把 baseline JSON 整体原样 PATCH；只发送 baseline 中的 URL、`content_type` 和 `insecure_ssl`，省略 `secret` 以保留 GitHub 端原值。实际签名校验使用既有 `/Users/lanzhijiang/.braid/instances/xiaoland/secrets.toml`（文件存在且权限 `0600`），不把值写入证据。

## 隔离启动封装

启动脚本为 [`start-github-wrangler.sh`](./start-github-wrangler.sh)，实际入口是 WorkSSD 上的 Node 与 Wrangler：

`/opt/homebrew/bin/node /Volumes/WorkSSD/Development/braid-e2e/wrangler/node_modules/wrangler/bin/wrangler.js tunnel quick-start http://127.0.0.1:18180 --log-level info`

脚本外层固定 `TMPDIR`、`XDG_CONFIG_HOME`、`XDG_CACHE_HOME`、`WRANGLER_LOG_PATH` 到 `/Volumes/WorkSSD/Development/braid-e2e/wrangler/{tmp,config,cache,logs}`，并设置 `TUNNEL_TRANSPORT_PROTOCOL=http2`。脚本已通过 shell 语法检查，但尚未执行；Wrangler help 已确认 `tunnel quick-start <url>` 命令契约。

## 待实际运行记录

只有内建阶段完成并交接 GitHub Console 后，才可在已授权窗口执行以下顺序：保存当前 App hook → 启动脚本 → 读取公网 `trycloudflare.com` 地址 → 以带签名请求验证 Braid ingress → 记录具体 HTTP 状态和响应摘要 → 临时写入 App-wide webhook → 验证 GitHub 事件到达 → 停止隧道 → 写回基线 JSON 中的 URL、secret、content type 和 insecure SSL → 读回确认恢复。

在实际启动前尚未有公网 URL、DNS/TLS、cloudflared protocol negotiation、签名请求或 App webhook PATCH 结果；以下记录是启动后的真实结果。

## 本次网络上线结果

- GitHub Console 已由 runtime owner 确认 `ready=true`，本地 health 返回 HTTP 200，ingress `127.0.0.1:18180`，reconciliation 为 `current`。
- Wrangler 首次启动触发了 WorkSSD 内的 cloudflared 下载；第一次短进程只留下下载检查日志，随后第二次启动成功。下载、配置与日志均在 `/Volumes/WorkSSD/Development/braid-e2e/wrangler`，未落系统盘。
- cloudflared 版本 `2026.10.0`，启动日志确认 `Initial protocol http2`、`Registered tunnel connection ... protocol=http2`。Connectivity pre-check：DNS、TCP HTTP/2 与 Cloudflare API 通过；region2 QUIC 失败，但最终明确 `degraded transport` 并使用 `http2`，符合本轮固定协议要求。
- 已通过公网 Quick Tunnel 对 `/webhook` 发送带本地既有 secret 计算的 `X-Hub-Signature-256`，实际 HTTP 状态 `202`，Braid 返回 `event_lifecycle=consumed`。请求体、响应、delivery UUID 和 URL 均仅保存在 WorkSSD `github/network` 下并设为 `0600`；敏感 URL 不写入本报告。
- App-wide hook 已 PATCH，仅发送公网 URL、`content_type=json`、`insecure_ssl=0`，没有发送掩码或本地 secret。GitHub 200 回执已保存，readback 确认 URL 存在、secret 仍为掩码、字段值正确。
- 已通知 runtime owner 可以开始正常 GitHub Issue/PR 需求输入。tunnel 与 App hook 在 GitHub Harness 结束前保持运行，不能提前恢复。

## Delivery 404 调查与修复

- Issue #37 `opened` 与 maintainer comment `created` 均由 GitHub 真实投递，但 App delivery 只读回执分别为 HTTP `404`。原始 delivery 列表、App 订阅和 installation 权限响应保存在 `github/network`，权限为 `0600`。
- App 订阅包含 `issues`、`issue_comment`、`pull_request`、`pull_request_review`、`pull_request_review_comment`；installation 权限为 issues/contents/pull_requests 写、metadata 读，排除了订阅或权限缺失。
- 根因是第一次临时 PATCH 把公网 URL 写成了 host 根路径；Braid ingress 实际路由是 `/webhook`。手工签名验证使用了 `URL/webhook`，所以不能证明 GitHub App 根路径正确。
- 已将 App URL 修为同一公网地址加 `/webhook`，仍只发送 URL、`content_type=json`、`insecure_ssl=0`，没有发送 secret；GitHub 200 回执及 readback 均确认路径正确、secret 仍为掩码。没有 redeliver 旧事件，也没有在调查中新增对象。
- 修复后的验证需要 runtime owner 在 root 决策后产生一个新的唯一 GitHub 事件；当前 network owner 不自行制造事件。tunnel 保持运行，等待该验证后再恢复 App 配置。

## 路径修复后的官方 delivery 结果

- 新 comment `6107449747` 的 GitHub 官方 delivery 已命中正确的 `/webhook` 路径；delivery detail 中请求 URL 已脱敏保存，确认不是根路径误投。
- GitHub delivery 状态为 HTTP `503`，响应正文为 `durable ingest unavailable`；响应 headers、delivery GUID、事件身份与请求 headers 已保存在 WorkSSD 原始 JSON，签名字段不在报告中展开。
- GitHub App 当前订阅与权限仍正常，hook readback 仍为 `/webhook`、JSON、`insecure_ssl=0`，secret 掩码存在；cloudflared 进程仍存活并使用 HTTP/2。
- Console 原始日志给出本地根因：`SQLite operation failed: UNIQUE constraint failed: repositories.name_with_owner`。这证明公网/App 路径已通过，当前阻塞是 Braid ledger durable ingest 的 repository 去重/幂等故障，交由 runtime/root 处理；网络 owner 不重投递、不新增事件、不改订阅或 secret。
- 追加身份核对发现：此前用于验证公网签名的本地 ping 使用了合成 `repository.node_id=MDEwOlJlcG9zaXRvcnkxMjM0`，而真实 GitHub 仓库 `xiaoland/braid-poc-test` 的只读元数据为数字 id `1344679384`、`node_id=R_kgDOUCYt2A`。`full_name` 相同但 node ID 不同；原 ping 未使用真实仓库身份，可能污染本轮 ledger 的 repository 记录。原 payload 保留在 `github/network/signed-body.json`，没有重发或修改。

## 受控 tunnel 中断

- 在 runtime 已达到 ready、provider connected、reconciliation current 且 PR/应用状态已冻结后，保存了中断前状态，停止了 Wrangler/cloudflared 进程；没有停止 GitHub Console、18180 ingress 或 health/reconciliation。
- 中断后本地 health 仍 HTTP 200，`ready=true`、`reconciliation=current`；公网原 URL 探测返回 Cloudflare HTTP `530`，正文为 Cloudflare Tunnel error 页面，证明公网投递入口已不可达。
- App webhook URL 与 secret 保持不变，未触碰 GitHub 配置；恢复窗口等待 root 明确指令。中断前状态文件、探测响应和进程快照保存在 `github/network`，均未包含 secret/token。
- 中断窗口内 PR #38 comment `6107701629` 的官方 delivery 只读详情为 HTTP `530`，状态 `Invalid HTTP Response: 530`，响应为 Cloudflare Tunnel error（错误码 1033）；delivery GUID、headers、完整响应原件已保存在 `github/network`，报告不展开公网 host。该结果确认中断确实影响 GitHub 投递，未执行 redelivery。

## 中断后恢复

- 恢复时后台 `nohup` 封装会随命令会话退出，导致其新 Quick Tunnel 地址再次返回 530；未修改 Console 或 ledger。随后改用受控长期终端会话运行同一 WorkSSD 启动脚本，cloudflared `2026.10.0` 注册成功并明确使用 `http2`。
- App webhook 已更新为本次新 Quick Tunnel 地址的 `/webhook` 路径，仅发送 URL、`content_type=json`、`insecure_ssl=0`，没有发送 secret；GitHub 200 回执和 readback 均确认路径正确、secret 仍掩码存在。
- 新地址的公网 GET `/webhook` 返回 HTTP `405`（路由可达但方法不允许），本地 health 仍 HTTP 200、`ready=true`、`reconciliation=current`。未重发中断期间失败 delivery，未发送 synthetic payload。

## 最终恢复与关闭

- 收口前只读保存了 closed delivery `cc498440-c558-11f1-98bb-49512759d9a4`：GitHub delivery HTTP `202`，响应为 Braid accepted/pending 事件回执；原始 delivery detail 保存在 `github/network`。
- App hook 恢复 PATCH 从 baseline JSON 只取 `url`、`content_type`、`insecure_ssl`，没有读取或发送 baseline 的掩码 `secret`。GitHub 200 回执后 readback 与 baseline 三个非 secret 字段全部一致，secret 仍为掩码存在。
- 通过长期终端发送 Ctrl-C，Wrangler/cloudflared 均已退出；进程核对为无 tunnel 进程。恢复、530、delivery 失败和 protocol 日志均保留在 WorkSSD，未删除原件、未 redeliver、未发送 synthetic payload。
- 最终本地 health 端口在 Harness 停止后不可达；Console 的停止后历史回读和进程关闭由运行 Owner 分别取证，不能用 health 端口代替。App 配置已恢复到原始 baseline。
