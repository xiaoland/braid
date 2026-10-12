# Deployment and Operator Contract

This document owns packaged runtime, configuration, migrations, observability,
tunnel supervision, health, and black-box campaign operation. It does not grant
GitHub mutation authority by itself.

## Supported Shape

The first supported artifact is a signed/checksummed macOS arm64 `braid` binary.
It runs without Python, PDM, Cargo, or a source checkout. Linux x86_64 is the
next target. Runtime prerequisites remain explicit:

- Git and `gh` for Agent workflows;
- pinned Codex executable and authenticated provider home;
- Wrangler for the free Quick Tunnel acceptance path;
- GitHub App ID/private key/webhook secret and installation;
- optional OTLP endpoint; full campaign acceptance requires one.

Release artifacts include `braid`, `LICENSE`, `README`, default config example,
OTel Collector example for optional tail sampling, SHA-256 checksums, and
release notes generated from `CHANGELOG.md` under SemVer.

Pi 可使用外部基础 CLI 或 [锁定原生材料](../../native/pi/README.md)。需要
subagents/background 的共同结果与停止合同时，先安装材料并明确选用插件，
配置 `[[runtimes]].executable` 与 `native_runtime` 为同一安装的入口与 manifest。
`doctor` 校验文件身份，实际启动核对 live 能力；安装或升级不修改系统全局 Pi。

## Configuration

The instance `config.toml` is versioned independently from binary and
database schema. It
contains no secret values; secrets use file references or environment-variable
names. The main sections are:

- GitHub App/repository/handle, API version, webhook and reconciliation;
- the instance key under `[instance]`;
- Profile definitions/tags/default PR Profile/context byte budgets/status
  surfaces/provider/workspace/resources;
- scheduler quiet/count settings;
- `[[runtimes]]` adapter executables/version pins and provider home;
- `[[llm_providers]]` provider credentials (file references, not values);
- SQLite/runtime/worktree directories;
- loopback ingress/health ports and Wrangler path;
- OTLP endpoint/protocol, trace sample ratio, and incident override;
- log format and bounded health/status settings.

`braid config check` prints the effective non-secret declaration and validates
its static invariants. `braid doctor` runs filesystem, SQLite, GitHub, Codex
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
      worktrees/<repository>/<pr-number>-<assignment-generation>/
    provider/                   # adapter home (e.g. Codex provider state)
    logs/                       # optional sampled-log export
```

The default `runtime.root` is `<config_dir>/state`. `braid setup` writes
`instances/<key>` explicitly with the layout above. Multiple instances share
the same user root but have isolated databases, webhooks, and worktrees.

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

Sampling controls volume, not confidentiality. An OTLP backend receives the
same sensitive material as the runtime and is configured/retained accordingly.
The portable binary does not promise outcome-aware tail sampling. Operators who
need “all errors plus 10% normal” route 100% from Braid to an OpenTelemetry
Collector tail-sampling processor; the Collector remains optional external
infrastructure.

The selected Rust SDK behavior was compiled and measured on 2026-08-13:
parent-based 10% sampling exported 945 of 10,000 independent traces, and every
retained span preserved all five controlled full-payload attributes.

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

Quick Tunnel URLs are temporary. A graceful stop restores the prior App webhook
URL. If the supervised Wrangler child exits unexpectedly, Braid immediately
marks tunnel health unavailable and attempts the same restoration while the
local runtime and reconciliation remain available. A failed repair stays
explicit in health and requires operator action. GitHub does not automatically
redeliver failed webhooks; reconciliation is the recovery path.
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

## Health and Status

Loopback `/healthz` and `braid status` expose bounded, non-semantic state:

- binary/config/DB/protocol versions;
- owner lease and migration state;
- GitHub/tunnel/reconciliation freshness;
- provider connection and unknown-turn count;
- active/debouncing/blocked group counts;
- oldest pending/uncertain write intent;
- OTel exporter and sampling mode.

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
