# Deployment and Operator Contract

This document owns packaged runtime, configuration, migrations, observability,
tunnel supervision, health, and black-box campaign operation. It does not grant
GitHub mutation authority by itself.

## Supported Shape

The supported packaged artifacts are signed/checksummed macOS arm64 and
Linux x86_64 `braid` binaries produced by the release workflow from the same
locked source. They run without Python, PDM, Cargo, or a source checkout.
Runtime prerequisites remain explicit:

- Git and `gh` for Agent workflows;
- pinned Codex executable and authenticated provider home;
- Wrangler for the free Quick Tunnel acceptance path;
- GitHub App ID/private key/webhook secret and installation;
- optional OTLP endpoint; full campaign acceptance requires one.

Release artifacts include `braid`, `LICENSE`, `README`, default config example,
OTel Collector example for a queryable OTLP backend, SHA-256 checksums, and
release notes generated from `CHANGELOG.md` under SemVer.

Pi 可使用外部基础 CLI 或 [锁定原生材料](../../native/pi/README.md)。需要
subagents/background 的共同结果与停止合同时，先安装材料并明确选用插件，
配置 `[[runtimes]].executable` 与 `native_runtime` 为同一安装的入口与 manifest。
`doctor` 校验文件身份，实际启动核对 live 能力；安装或升级不修改系统全局 Pi。

## Configuration

The instance `config.toml` is versioned independently from binary and
database schema. It contains no secret values. Provider credentials always use
explicit `credential = { kind, file }` references (Braid-schema `api_key` /
`oauth` files, or `none`); the GitHub webhook secret separately supports a file
reference or environment-variable name. The main sections are:

- GitHub App/repository or explicitly configured multi-repository set,
  handle, API version, webhook and reconciliation;
- the instance key under `[instance]`;
- Profile definitions/tags/default PR Profile/context byte budgets/status
  surfaces/provider/workspace/resources;
- scheduler quiet/count settings;
- `[[runtimes]]` adapter executables/version pins and adapter home;
- optional `[[llm_providers]]` for Pi profiles: explicit `credential` (file
  references, not values) plus `base_url`/`api`/`models` declarations from which
  the Pi adapter derives `models.json`; Codex profiles do not declare them;
- SQLite/runtime/worktree directories;
- loopback ingress/health ports and Wrangler path;
- OTLP endpoint/protocol, trace sample ratio, and incident override;
- log format and bounded health/status settings.

`braid config check` prints the effective non-secret declaration and validates
its static invariants. Single-repository GitHub configs may omit
`profile.repository`; a config with `github.additional_repositories` requires
every Profile to declare its repository and explicit workspace. The App and
webhook secret stay instance-level, while Profiles, scheduling, worktrees, and
writes remain repository-scoped. `braid doctor` runs filesystem, SQLite,
GitHub, Codex
schema, Git/gh, OTLP, and Wrangler precondition probes. Profiles expose
their effective mapped provider settings through `braid profile inspect`.

Pi Profile 的可选技能、原生模板、独立 home 和有效身份操作见
[Profile 指南](../user-manual/profiles.md)。协作技能包独立交付，不属于默认运行材料。

## Filesystem Layout

Braid keeps one user root outside repository worktrees, default `~/.braid`.
Everything below is scoped by trust boundary:

```text
~/.braid/
  registry.toml                 # instance registry (SSoT)
  config.toml                   # optional user defaults (future)
  secrets/                      # user-level provider API keys
  instances/<key>/
    config.toml                 # Harness config, schema version 3 (GitHub v2 still readable)
    github-app.pem              # GitHub App private key
    secrets.toml                # webhook secret
    state/
      braid.sqlite3
      backups/
      worktrees/<repository>/<issue-or-pr-number>-<assignment-generation>/
    provider/                   # adapter home (e.g. Codex provider state)
    logs/                       # optional sampled-log export
```

The default `runtime.root` is `<config_dir>/state`. `braid setup` writes
`instances/<key>` explicitly with the layout above. Multiple instances share
the same user root but have isolated databases, webhooks, and worktrees.
A provider credential file is Braid schema (`provider_api_key = "..."` for
`api_key`, or an `access`/`refresh`/`expires` token set for `oauth`) and must
be mode `0600`. Its path and provider id are explicit user choices. The Pi
adapter converts it at session materialization — injecting `api_key` as the
managed environment reference or rendering `oauth` into the session home
`auth.json`; `kind = "none"` injects nothing. Runtime homes never carry
`auth.json`; convert existing ones once with `braid provider import-auth`.

## Database Lifecycle

`braid migrate plan` reports pending embedded migrations and compatibility.
`braid migrate apply` takes the exclusive migration lease, creates a timestamped
pre-migration backup, verifies historical checksums, and applies transactions.
`braid serve` may apply migrations only when `auto_migrate = true`; otherwise it
refuses with the exact command required.

There are no production down migrations. Release notes declare the oldest
binary compatible with the resulting schema. Rollback across an incompatible
migration restores the stopped pre-migration DB backup into a distinct runtime
directory.

## OpenTelemetry

Braid emits traces, logs, and metrics over OTLP. Metrics are never probabilistic
sampled. Trace roots cover one GitHub delivery/reconciliation change through
Context, scheduling, provider, and Braid-owned GitHub convergence.

The Rust SDK uses parent-based trace-ID ratio head sampling. Default ratio is
`0.10`; acceptance and incident mode set `1.0`. Sampling is decided once at the
root so retained traces do not contain arbitrary middle gaps. Payload-bearing
evidence—including GitHub bodies/summaries, raw webhook, provider transcript,
credentials, API results, and local paths—is attached only to recording spans
as log/event bodies rather than high-cardinality metric labels.

Full sampled payload events use the dedicated `braid::evidence` target and go
only to OTLP logs. They are not copied into trace events or ordinary Text/JSON
stderr/journal. JSON formatting does not inherit span fields; evidence must
never be placed in ordinary span attributes. Provider chunks do not generate
per-chunk ordinary summary logs: lifecycle/session diagnostics and the low-cardinality
payload sampling/identity counters provide coverage without journal amplification.

Sampling controls volume, not confidentiality. An OTLP backend receives the
same sensitive material as the runtime, including credentials and complete tools'
output, without truncation. Backend access, backups, retention and destruction
must be authorized for that data. Do not replace evidence with incident-only
logging or a debug exporter. The default Collector example forwards all three
signals to a queryable LGTM-compatible OTLP backend and does **not** sample
again. Default Braid `0.10` therefore remains 10%, not 1%. Use
`sample_ratio = 1.0` during a controlled dogfood/acceptance window. Optional tail
sampling requires a deliberately different policy with Braid at 100%; a
trace-only tail sampler does not automatically sample correlated logs.

### Disposable loopback verification backend

From a checkout, with a **local** Docker daemon and Compose:

```sh
docker compose -p braid-observability -f deploy/compose.observability.yaml config --quiet
docker compose -p braid-observability -f deploy/compose.observability.yaml up -d
```

The version-pinned LGTM container stores logs in Loki, traces in Tempo, and
metrics in Prometheus. The separate Collector uses
`deploy/otel-collector.example.yaml`, not stdout/debug export. Set the instance:

```toml
[telemetry]
endpoint = "http://127.0.0.1:14318"
sample_ratio = 1.0
incident_mode = false
```

Do not overwrite other existing telemetry keys. Run:

```sh
braid telemetry probe --config /path/to/config.toml --marker DOGFOOD2_CONTROLLED_PROBE --json
curl -fsS http://127.0.0.1:18888/metrics | grep -E 'otelcol_(receiver|exporter)_.*(accepted|refused|sent|failed)'
```

`probe` reports the trace ID plus per-signal endpoint and SDK export call
success/failure counts after flushing. `payload_emitted` means submitted to the
logging layer, not receipt. `successful_batches` means the SDK exporter returned
success, not a durable backend acknowledgement. Partial rejection, queue drops,
retry counts and HTTP/connection sub-stages are not exposed by this SDK boundary;
`queue_dropped_records: null` means unknown, **not zero**. Failure diagnostics
state endpoint/signal/stage and records in failed calls without recursively
exporting themselves or printing remote response bodies. A metrics record here
counts one ResourceMetrics envelope, not individual datapoints. All signals are
flushed even if one fails; optional OTLP failure does not fail business work.
`doctor` connectivity and one successful probe do not prove sustained delivery.
Monitor Collector receiver refusal/export failure and backend query results
throughout a real turn.

Open **http://127.0.0.1:3300**, Grafana login `admin` / `admin` (local test only),
then Explore:

1. Loki: `{service_name="braid"} | trace_id="<probe trace_id>"`.
   If the configured service name differs, use it instead. Structured metadata
   includes `trace_id` and normalized OTLP attribute names. To find a session,
   use `{service_name="braid"} | braid_provider_session_id="<native-session-id>"`.
   Open the matching record and inspect `provider_transcript` / `credential` /
   `github_webhook_payload`; their complete values live in log attributes,
   not the short message body. Record the trace ID from the session log.
2. Tempo: select trace-ID query and paste that ID. Verify root/child spans
   and session/turn association; payloads intentionally are not span events.
3. Prometheus: query `braid_telemetry_probes_total` after a probe, or
   `braid_provider_payloads_total` during a turn. Metrics are not sampled.

Only host loopback ports 14318 (ingest), 18888 (Collector internal metrics),
and 3300 (Grafana UI) are exposed; backend ports stay on the private Compose
network. **Do not expose these ports through tunnels or bind to public IPs.**
Local users/Docker administrators can read evidence; this template has no
multi-user security boundary. The images and writable container layers are
for a short, supervised local check, not a retention-managed production store.
Loki has a 24h retention policy (asynchronous deletion), plus explicit 64MB
structured-metadata and 16MB body limits with truncation disabled. Oversized
records are rejected, not partially retained; monitor rejected records. There
are no host data mounts or configured persistent volumes. Stop and destroy
all evidence after the window:

```sh
docker compose -p braid-observability -f deploy/compose.observability.yaml down -v
```

A mere `stop` retains sensitive writable layers. For durable deployment, use an
operator-controlled LGTM-compatible OTLP backend with explicit authenticated
access, retention/deletion, storage capacity, backup policy and TLS for any
non-loopback hop; change `otlphttp/lgtm.endpoint` accordingly. Logs must accept
complete large attributes without truncation: validate by reading a long tool
result back before trusting a backend's limits. Remote Docker contexts require
host-local files and deliberate port access; do not mistake their daemon's
loopback for the CLI machine's loopback.

### Managed single-host dogfood backend

The disposable template above must **not** receive real sensitive evidence.
For a controlled dogfood window, use the small
`deploy/compose.observability.dogfood.yaml` override. This is a Braid-owned,
retained resource, not task temporary storage. It is not production multi-tenant
security: host users with access to the private directory and Docker admins can
read all evidence. Only Grafana queries are authenticated; ingest and Collector
metrics remain host-loopback-only. Keep backend ports private, never publish a
public tunnel; remote UI access may use an operator-only SSH loopback forward.

Install the exact committed templates into a private managed directory, e.g.
`~/.braid-inkcre/observability/dogfood2` (not another project's stack/volume):

```sh
umask 077
managed="$HOME/.braid-inkcre/observability/dogfood2"
mkdir -p "$managed"
chmod 700 "$managed"
cp deploy/compose.observability*.yaml deploy/otel-collector.example.yaml \
   deploy/loki.observability.yaml deploy/start-dogfood-lgtm.sh "$managed/"
cd "$managed"
# Only the managed copy changes; full-field/reject-not-truncate limits stay intact.
sed 's/retention_period: 24h/retention_period: 168h/' loki.observability.yaml > loki.tmp
mv loki.tmp loki.observability.yaml
# Generate once, never print or commit. Grafana reads via __FILE, not environment value.
test -e grafana-admin-password || openssl rand -hex 32 > grafana-admin-password
chmod 600 grafana-admin-password
# The Collector runs as a non-root container UID; bind-mounted config must be readable.
# The enclosing managed directory remains 0700; these templates contain no secrets.
chmod 644 otel-collector.example.yaml loki.observability.yaml start-dogfood-lgtm.sh
# Use this same project + both files for every lifecycle command.
docker compose -p braid-dogfood2 -f compose.observability.yaml \
  -f compose.observability.dogfood.yaml config --quiet
docker compose -p braid-dogfood2 -f compose.observability.yaml \
  -f compose.observability.dogfood.yaml up -d
```

The override disables anonymous access and signup, mounts the mode-0600 password
as a file secret, sets `unless-stopped`, and persists `/data` in the dedicated
`braid-dogfood2_data` volume. Local Compose secrets are file mounts, not encrypted
secret storage. Before routing real evidence, verify unauthenticated Grafana
`/api/datasources` returns **401**, authenticated queries work, mounts and file
modes are correct, and restart/recreate retains a synthetic probe. Grafana login
is `admin`; obtain the password directly from the private password file, never
paste it into issue comments, command output, or model context. Password changes
after initialization require Grafana's admin rotation procedure; replacing the
bootstrap file alone does not rotate the database password.

Default managed retention is **7 days**: installed Loki copy uses `168h`, the
pinned-image startup wrapper sets Tempo block retention to `168h` and Prometheus
TSDB retention to `7d`. Deletion is asynchronous (not a hard erasure deadline),
and WAL/head data may outlive the nominal query window. Grafana configuration
persists until explicit destruction. Verify actual config/flags after upgrades;
the wrapper is tied to LGTM 0.11.0. No second Collector sampling or debug exporter.
The original disposable template remains 24h Loki / ephemeral layers.

During dogfood, check daily and after failures: host `df -h`, volume `du`, container
restart/state and bounded logs, Collector receiver refusals/export failures, Loki
rejections and actual Loki/Tempo/Prometheus query results. Investigate any nonzero
failures or missing probe; stop/shorten the window before disk exhaustion (reserve
at least 20% free). Restart policy is not a per-component watchdog: LGTM may stay
running after an internal process dies. Check backend readiness and queries, not
container state alone. No automatic alerting or backup is supplied by this small
single-host stack. The operator owns these checks. Backups contain raw secrets and
transcripts: keep access as restrictive as live storage, encrypt off-host copies,
record their own expiry, and authorize restoration/destruction. For a consistent
volume backup stop the stack first; never treat a live filesystem copy as one.

`stop` preserves evidence for the next window. For authorized final destruction:

```sh
cd "$HOME/.braid-inkcre/observability/dogfood2"
docker compose -p braid-dogfood2 -f compose.observability.yaml \
  -f compose.observability.dogfood.yaml down -v
# After separately handling authorized backups, delete the private managed files.
rm -f grafana-admin-password
```

Destroying this volume does not erase separately saved backups or host snapshots.
Do not remove existing Velune or other Braid Collector resources. Disposable
verification projects must separately run their original `down -v` at task close.

## Startup and Supervision

Startup is ordered:

1. parse/validate config and secrets;
2. acquire process/runtime owner lease;
3. verify/apply permitted DB migrations;
4. probe installed Codex version/schema and GitHub App installation;
5. start OTLP exporters and loopback health/ingress;
6. start Codex adapter and reconcile persisted sessions/turns/outbox;
7. start canonical GitHub reconciliation;
8. optionally start Wrangler, verify a signed public ping through the returned
   URL, then update the GitHub App webhook;
9. mark repository ready for assignment/turn claims.

The provider adapter reconnects and resumes compatible sessions where the
provider contract makes that safe. Any other supervised worker exit makes
readiness false and stops the runtime rather than leaving a partially live
process. A provider disconnect never invents an Agent terminal state.

Shutdown first stops new claims, then gives workers, the GitHub outbox, ingress,
health, and OTel bounded deadlines before releasing the owner lease and closing
SQLite. An active turn without provider terminal evidence remains `unknown` on
the next compatible resume; Braid neither starts a parallel turn nor calls it a
success or failure.

Quick Tunnel URLs are temporary. A dedicated supervisor owns the child, public
verification and App URL synchronization independently of Agent execution. Child
exit or an explicit `Tunnel not found` marks connectivity unavailable and starts
bounded, backed-off candidate rebuilding without restarting `serve` or its Agent
sessions. Every 60 seconds it verifies a signed public request receives ingress
202 and that the App still names that URL. A failed check reports degraded
immediately; three consecutive public failures replace the candidate. URL drift
retries synchronization of the live candidate rather than creating a new Agent
session. Failed App synchronization remains `sync_pending` and retries with a
10–120-second backoff. On Unix, each candidate starts in its own owned process
group, including Wrangler's Node wrappers and cloudflared. Stop and failed
registration close that group; cancellation retains a last-resort group signal.
An unconfirmed stop reports `stop_unconfirmed` and stops creating candidates
until operator recovery. It does not restart Agent execution to hide the fault.

`/healthz` returns 503 while a requested Tunnel is not connected. Its
`webhook_sync`, `tunnel_verified_at` and `tunnel_error` distinguish the last
confirmed URL from a pending or failed repair; systemd active is not webhook
readiness. `doctor` primarily checks installation preconditions and cannot
substitute for these live observations. On graceful stop, Braid restores the
prior App webhook only if the current URL still belongs to its own candidate.
GitHub does not automatically redeliver failed deliveries: canonical
reconciliation recovers current-state changes, while `github redeliver` remains
the explicit route for recovering a particular delivery.
On networks that block outbound QUIC, the Quick Tunnel supervisor explicitly
uses Cloudflare's TCP HTTP/2 transport. Readiness requires both cloudflared's
registered-connection signal and a signed request that traverses the public URL
back into Braid's verified ingress. A registered but publicly unreachable
account-less tunnel is discarded before any GitHub mutation; Braid retries a
bounded number of fresh tunnel candidates and remains unready if none verifies.

GitHub exposes no API that creates the App webhook configuration from its
disabled state. The dedicated App therefore needs a one-time Human-confirmed
bootstrap in App settings: enable the webhook with a non-routable baseline URL
and stable secret, then select the events declared in the GitHub boundary
contract. Once that hook exists, `serve --tunnel` reads its prior URL, patches
the verified temporary URL, and restores the prior URL on graceful shutdown.

### 升级现有实现请求与原生材料

Schema 12 会把“已固定 PR，但 activation receipt 缺失”的旧错误 applied ensure 重新
交给原 outbox。迁移本身不启动 Agent；服务恢复后，执行 owner 重验当前精确 PR、
源 Issue 的 OPEN 状态、canonical 关联、Profile 与生命周期，再补交原授权。已有
blocked/sleeping/retired/finalizing 历史不会被 ensure 自动解阻或换代。升级前应检查
这些待恢复请求，尤其旧版本已创建却从未交接的 PR；不要把它误解为按任意关联自动
授权新角色，也不要把 applied 当成模型已执行。

Braid 二进制升级不会替换配置引用的 immutable Pi 安装。新版本源码中的环境修复
只有在新材料实际被引用后才生效；禁止原地改动 manifest 已锁定的安装。保留旧材料
时，可由服务显式设置正确的 `GH_CONFIG_DIR` 作为过渡，路径不能由 Agent 猜测。
材料 identity 检查和 `pi --version` 不证明 execution-local 环境下的默认 gh/Git
credential helper 可用；安装验收要在那个环境中验证，仅报告结果，绝不输出 token
或 credential 的值。

切换服务配置前，用候选 binary 对候选配置执行 `braid doctor --config PATH`。
只读 Session compatibility 检查将当前有效材料与最新既有 generation 比较，并对
可静态重建的 GitHub 工作项校验指令。Built-in 按已显式选择的 Profile 比较材料；
有限 run 的动态协调指令仍由运行时校验，预检不猜测它。不更改 assignment、Agent、
Session 或分支。材料/指令不匹配会明确失败；材料回退后
仍 blocked 或执行 unknown 也不会被当成已恢复。先保持兼容配置，或明确用
`assignment reactivate` 创建新 generation。恢复完全相同 Profile 只是复用旧 generation
的必要条件，不是 owned execution 已停止的证明；当前不提供把旧 blocked 记录自动
翻回 active 的通用入口，不把现场特定的 SQL 逆向操作当作受支持恢复流程。

Schema 13 只修复 Operational Status 的公开投影。已有 generation 的正常运行证据
可在服务恢复、outbox 下一次领取时，把旧故障 comment 更新为已恢复或由新代接替，
无需再次激活或调用模型；原 blocked 记录不会改变。更新包含最后确认时间，不能
当作当前凭据仍有效或工作已验收的证明。若旧评论创建尚未确认，执行器先对账原
请求再更新，避免重复 comment。OAuth 源文件生命周期不在这次迁移范围内。

## Health and Status

Loopback `/healthz` and `braid status` expose bounded, non-semantic state:

- binary/config/DB/protocol versions;
- owner lease and migration state;
- GitHub/tunnel/reconciliation freshness;
- provider connection and unknown-turn count;
- active/debouncing/blocked group counts;
- oldest pending/uncertain write intent;
- OTel exporter and sampling mode.

`runnable_batches` remains the compatible raw batch-lifecycle count, not a claim
or execution promise. `durable_eligible_batches` and each batch's `scheduler`
projection use the same assignment/agent/session conditions as durable claim:
active + idle agent, or finalizing + finalizing agent, with an idle provider
session. Reasons distinguish non-runnable, no active/finalizing assignment,
no matching agent and no idle session. Physical provider availability is explicitly
`unknown_not_in_database`; claim additionally filters the worker's available
physical sessions and requested kind/Profile. Closed/merged alone is not an
exclusion: finalization can remain eligible. Diagnostics never delete/consume
residual batches or change Group lifecycle. Agent groups mark `current_generation`
by the highest durable assignment generation for that Work Item; historical blocked
records are not current-generation failures. Both JSON and text expose this split. A deferred complete-input read also keeps
its bounded `input_error` on the batch, so a runnable lifecycle cannot disguise
repeated Context failures. Native execution has not started for that input.

Health never reports task acceptance, design readiness, or implementation
success. Operational Status Comments are the GitHub-visible projection for
specific blocked Work Items.

## Real Campaign

Install the candidate artifact into a clean directory, create a private runtime
root, configure the dedicated App/Profiles/OTLP endpoint, and run:

```shell
braid --version
braid config check --instance <KEY>
braid doctor --instance <KEY>
braid migrate plan --instance <KEY>
braid migrate apply --instance <KEY>
braid serve --instance <KEY> --tunnel
```

Then drive only the real campaign in
[`../10-prd/acceptance.md`](../10-prd/acceptance.md). Source-tree execution,
private DB edits, fake events, or a manually signed local request cannot accept
the release.

## 内建平台与 Harness 接入

内建平台使用独立声明、schema 与数据库，作为 Harness 的上游与 GitHub 并列。
`braid platform init/serve/import` 管理平台；`braid init` 只声明 Harness 执行环境
与 `[builtin]` repository/endpoint，不创建 GitHub App 或保存 API key 值。
平台 source 与默认分支独立绑定，不能通过修改 endpoint 静默切换空间身份。

Harness 配置 schema 3 选择 `[github]` 或 `[builtin]`。原 GitHub schema 2 声明
继续有效，旧 `[local]` 明确拒绝，并给出显式平台导入指引。导入前停止旧进程
和写入入口，核实在途意图；保留原数据库、对象身份、事件及对象回执。Harness
数据库迁移至 schema 6，平台此后使用自己的 schema，不读取执行表。迁移及
启动命令见[内建平台指南](../user-manual/local.md)。

平台当前 schema 2 持有自己的嵌入迁移名称与 SHA-256 校验。旧平台 schema 1
在启动时先备份再向前升级，保留空间身份、对象和回执；不通过重建空库升级。
未知版本或迁移校验不一致明确拒绝。历史候选与合并提交保留平台专属 Git 引用，
避免工作分支移动或清理后丢失已接受的提交。

两个上游经过同一 Harness 运行入口、lease、调度和 Group/provider 停止合同。
内建 adapter 使用 HTTP 平台接口，事件 cursor 和 grant 能力保留在 Harness
ledger；平台不为每个消费者修改事件。`--transport-only` 接入对象事件和队列，
不启动原生模型会话。GitHub 专属命令在内建上游明确拒绝。

Agent 子进程取得 `BRAID_CONFIG` 与 `BRAID_AGENT`，对象 gateway 通过专属 cwd、
当前 assignment/session/turn 选择有效 grant；平台提交事务核验授权版本。撤销
失败传播并阻断执行替换，不把一次 gateway 读取当作提交时仍有效的证明。同
账户进程不是安全沙箱的隔离对手。平台对象、provider history 与 Git 资料保留
各自身份和数据库，停止 Harness 不删除平台。

## Braid Collector 与 Console

`braid console` 提供 loopback OTLP HTTP/protobuf、独立原始证据库和内嵌 Web。
`--config` 可连接任一上游的 Harness；`--platform-config` 可便捷托管独立平台，
也可只配置平台而不配置 Harness。对象操作经过固定平台 API，执行 snapshot
只读 Harness ledger，runtime manager 持有真实任务与启停回执。Console 不走
CLI 子进程，也不由 OTLP 重建权威。停止 Harness 后平台和 Collector 继续在线。
使用方法和未确认停止的处置见[Console 指南](../user-manual/console.md)。

平台命令回执属于平台数据库，启停回执属于 Harness。普通对象变更、事件和
回执原子提交，Git 候选保留自己的意图与 ref CAS。监督器等待所有 Group/provider
停止结果，任何未确认停止进入 blocked 并禁止替代启动。Console 可在同进程
装配三项职责，但不因此合并权威，亦不提供进程级故障隔离。

真实指标使用低基数标签：runtime 观察代次开始/结束次数、provider 生命周期事件计数、接纳到
终态的单调时钟观察时长，以及 provider payload 的采样和关联覆盖计数。时长是
观察区间，Unknown 不是停止证明；没有原生 usage 事实时不推测 token 或费用。

OTLP 信号接口的失败响应按[协议规范](https://opentelemetry.io/docs/specs/otlp/#failures)
使用 protobuf Status；正常批次因存储不可用不能接受时返回 503，让 exporter 可以
重试。成功响应只表示持久接收，不承诺 Exactly Once；Web 查询和控制错误另用
JSON，避免浏览器把诊断响应当作业务回执。

## GitHub GraphQL quota recovery

正常预算下，OPEN 或仍有 materializing/active/finalizing Assignment 的 Work Item
完整巡检成功后默认 60 秒再次到期；其他已关闭/合并历史项为 30 分钟。
`scheduler.reconciliation_seconds` 控制 hot 成功间隔，不是 worker tick；本地
到期检查每 `min(reconciliation_seconds, 5)` 秒执行，另有遍历、网络与预算等待。
失败项至少间隔一个 hot 周期再试；已知 installation 冷却期间不会反复联网，
巡检只在进入降级状态时告警。

升级需应用 schema 8 迁移，新增独立的完整成功/尝试时钟；旧记录起初均到期，
但仍受 installation 准入限制。重启应保留 runtime 数据库，避免丢失成功事实。
旧二进制不能直接打开升级后的数据库；回滚遵守上文 Database Lifecycle。

普通成功预算事件已降为 DEBUG。以下指标/归因日志的 coverage 仅为 Braid GraphQL
transport（包括 canonical Context 完整遍历），不是所有 GitHub 调用。REST（含
`write_rest`）及 App/installation-token 请求的请求数、时长和成本仍为 unknown。
按 `braid.github.requests`（GraphQL HTTP请求，不是points）与
`braid.github.request.duration` 聚合 operation/class/resource/outcome/cost_known，
其中 metric 的 resource 固定为 `graphql`；
`braid.github.graphql.points` 只统计实际返回 `rateLimit.cost`，不能用 unknown 样本
推导总点数。指标不带 repository/installation/object/session 标签；DEBUG/span 以
repository/work item、installation、request ID、status/latency、used/remaining/reset
归因，WARN 保留准入/限流/退避失败。used delta 在同一resource/reset窗口内才可比，
仍可能包含别的调用者。字段缺失明确 unknown，不能冒称 querycost。body/token/vars
不进入这些普通日志；完整采样证据的受控路由不变。

本地 mock GraphQL HTTP 单页、无项目/关联/线程/drift 的完整读取账单 Issue13→5、PR10→5；
关联Issue、线程评论、项目字段及分页另外计费。该下降不证明线上 points 同比例下降。
正常预算窗口需在真实backend对操作指标与限额事件联合观察，再决定后续优化。

后台在剩余点数不大于 100 时让出余量；主限额耗尽等 reset，次级限流遵守
Retry-After，缺提示的限流或网络故障有界退避，到期重新探测。不要反复重启绕过
冷却：预算只在进程内共享，重启后首次响应前未知，而 GitHub 的安装额度不会重置。
独立 CLI、其他进程和 REST/App token 操作不受此 GraphQL gate 协调。若活跃集合
本身超预算，应减少消耗或调整目标周期，退避不能创造额外吞吐。

**自动恢复范围有限。**巡检和仍待处理的请求可在冷却后继续；首次 setup/serve 的
身份读取失败发生在 worker 启动前，需要额度恢复后重试启动。已因 Context 物化
失败进入 `blocked` 的 Issue 激活或 Context replacement 不会被一次成功探测自动
解除；先排除权限、配置和外部故障，再使用既有 operator reactivation 路径。
不要把 API 配额恢复等同于所有 Agent 业务自动继续。
