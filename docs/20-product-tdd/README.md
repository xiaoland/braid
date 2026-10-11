# Product TDD: Working Memory Runtime

This document owns Braid's cross-unit authority, topology, state ownership, and
Rust realization. It deliberately excludes unit algorithms that are evident
from typed code and external wire details owned by the linked contracts.

## Admission

- **Dependent units**: GitHub ingress/API/reconciliation, Context projector,
  event classifier and scheduler, Agent Group/session manager, Codex adapter,
  worktree provisioner, Braid GitHub writer, durable store/outbox,
  observability, tunnel supervisor, and CLI/runtime.
- **Failure if lost**: GitHub edits could leave stale provider context active,
  direct Agent writes could feed back indefinitely, Issue and PR profiles could
  share the wrong memory/worktree, or a process restart could duplicate turns
  and GitHub mutations.
- **Why code is insufficient**: the contract crosses GitHub, provider,
  filesystem, database, telemetry, and Human-visible lifecycle authorities.

## Authority and Topology

```text
GitHub (object authority) ── webhook / canonical API ──┐
                                                      ├─> Harness collaboration adapter
Built-in platform (object authority) ── HTTP / events ┘             │
                                                                   v
                                                     Harness ledger / event cursor
                                                                   │
                                                Context / scheduler / Agent Group
                                                                   │
                                                          provider / worktree

Agent ── Harness gateway / writer grant ──> selected platform API
Harness outbox ── idempotent write ───────> selected platform API

Braid Web Console ── object API ─────────> built-in platform
                  ├─ runtime controller ─> Harness (either upstream)
                  └─ OTLP Collector ──────> independent observation store / visualization

Platform, Harness and observations each own their database and lifecycle.
Quick Tunnel is optional GitHub webhook connectivity, not a Harness mode.
```

- **GitHub** owns current Issue/PR bodies, metadata, relationships, comments,
  reviews, lifecycle, identity, and App permissions.
- **Built-in platform** independently owns its Issue/PR state, actors, event sequence,
  candidate commits and command receipts; its SQLite is collaboration authority.
- **Collaboration Context** is a deterministic, complete projection of upstream state plus
  durable deletion tombstones. It is the collaboration memory authority.
- **Provider** owns physical session data, model execution, provider-native
  compaction, turn lifecycle, and resume availability.
- **Coding Agent** owns interpretation, design/implementation choices, code,
  verification, and its Git/GitHub operations.
- **Braid Harness** owns canonical reads, projection, logical Agent Group/session
  generations, scheduling, Context fencing/replacement, provider addressing,
  PR worktree provisioning, Braid-authored upstream writes, and mechanical
  idempotency/reconciliation.

Harness SQLite stores enough canonical object metadata and lifecycle tombstones to
compare versions, but it does not become an alternate Issue/PR or provider
transcript authority. A full Context is rebuilt from the selected upstream before every turn.

## 协作平台与 Harness

一个 Harness 恰好接入 `[github]` 或 `[builtin]`。GitHub 和内建平台拥有上游
对象权威，不双写或自动同步。内建平台有自己的配置、schema、SQLite 与 API，
持有仓库、Issue/PR、评论、指派、关联、revision、候选、合并意图及对象回执。
平台不读取 Profile、assignment、session、turn 或 worktree 执行表。

Harness 的 `collaboration` adapter 从所选上游取得当前对象与默认分支，Group、
Context、queue、provider 和停止证明共用。内建 adapter 使用 HTTP 平台接口，
在自己的 ledger 保存平台身份、事件 cursor 和 delivery 去重。平台对象变化
与稳定事件原子提交，事件不因某一消费者读过而消失；Harness 成功 ingest 后才
推进自己的 cursor，状态 outbox 通过幂等接口写回平台。

平台 actor 与 Harness Profile 分别拥有含义。Harness gateway 核验当前工作区及
执行身份，使用平台 writer grant。平台提交事务检查 grant epoch、对象 revision
和 request ID；执行替换先撤销旧 grant，撤销确认后才能完成 fencing。平台
保存授权版本，不保存原生会话状态；撤销前已经提交的命令仍属于合法历史。

PR 交接固定 commit OID。Harness 验证工作树责任，平台只在绑定 Git 对象库中
验证提交并保留可达引用。首版要求两者能访问同一 Git 对象库，不提供远程
Git 托管。合并用持久意图与 ref CAS 处理 SQLite/Git 边界；模型 Completed 不
意味着候选被采用或 Issue 完成。旧 local:*对象与事件身份在显式导入时保留，
历史迁移不可变；平台与 Harness 的 schema 此后各自演进。

## Single Source of Truth for Schema

A schema must have exactly one authority. When the same shape is defined both
by typed Rust structs and by a hand-written text template, the two drift and
Humans lose track of which one is "real". The Rust type is the source of truth;
any file that claims to be a valid instance of that schema must be provable
against it.

- **`Config` owns `braid.toml`**: `src/config.rs` defines the single source of
truth for the config schema. `braid setup` builds a `Config` value and
serializes it, rather than formatting a template. If the type changes, the
compiler forces the generator to change with it.
- **Generated artifacts**: 配置由 canonical Rust type 序列化；编译与实际公开
  命令操作核实消费者能解析和使用产物，不维护测试入口。
- **No parallel string templates**: Do not maintain a separate text template
for a file whose shape is already defined by a Rust type. Avoid
`format!`-based generators for config, manifest, or protocol files.
- **ROI boundary**: Not every documentation snippet is a schema instance.
Only canonical or generated artifacts participate in SSoT validation. For
example, `config.example.toml` is the canonical starter template, so it is
loaded through `Config::load` by the public configuration command; a partial snippet in a runbook is not.
- **Migrations are the exception**: SQLite schema is forward-only, immutable,
and checksum-verified; it is not derived from Rust structs because the database
must outlive any single binary version.

## Rust Runtime Shape

MVP is one Rust package and one `braid` binary rather than a workspace of thin
crates. Modules are deep and align with authority boundaries:

| Module | Owned interface |
| --- | --- |
| `home` | User root resolution, instance registry, `~/.braid` layout, port allocation, and config-path precedence. |
| `config` | Versioned config/Profile loading, validation, effective defaults, and diagnostic projection. |
| `github::webhook` | Raw-body HMAC verification and typed open-enum webhook admission. |
| `github::client` | GitHub App auth, bounded typed GraphQL pagination, REST writes, reactions, and canonical rereads. |
| `context` | Canonical snapshot model, HTML-comment removal, deterministic Markdown rendering, budget, and revision. |
| `events` | Canonical diff classification and compact Event Reference rendering. |
| `store` | One dedicated SQLite actor, transactions, migrations, leases, ledgers, sessions, batches, and outbox. |
| `producer` | Webhook/GraphQL observation、canonical diff 与事件分类；独立的 `mentions` 循环补全 GitHub 权限事实并保留失败 backoff。 |
| `queue` | 独立推进 Quiet Window、count、urgent 与批次状态；通过 store 领取可运行 turn，不执行网络权限查询或持有会话事件 receiver。 |
| `outbox` | 独立收敛 GitHub reactions、comments、statuses 及 uncertain writes；runtime 关闭时执行最终 drain。 |
| `group` | `GroupDriver` 统一 Issue/PR 的逻辑生命周期、dispatch、Context replacement 和恢复；领域模块保留 Profile、prompt、worktree 与兼容性规则。`RunningAgentTurn` 保存当前 claim 和事件 receiver。 |
| `agent_session` | Core 定义的 `SessionFactory` 创建/恢复契约与 `AgentSession` 行为、事件、失效观察和释放契约。 |
| `group::session_manager` | 按 opaque provider session ID 索引中立句柄；只恢复缺失或失效的句柄，并释放不再使用的句柄。持久化绑定仍由 store 权威保存。 |
| `provider` | 实现 core 会话契约，拥有物理进程、连接、会话寻址和通知翻译。Codex factory 内部缓存共享 app-server；Pi factory 为每个物理会话创建独立资源。 |
| `worktree` | Validate a Profile source checkout, resolve the bound ref (PR head, sole Development branch, or default origin branch), provision one generation-scoped worktree per Agent Group, and expose recovery diagnostics; no Git-operation sandbox. |
| `writer` | `braid gh`, attribution, reaction/status desired state, and write-outbox convergence. |
| `telemetry` | Trace/metric/log creation, payload events, sampling configuration, and OTLP export. |
| `tunnel` | Wrangler Quick Tunnel supervision and webhook URL handoff. |
| `runtime` | Owner lease, worker supervision, boot-time configuration gates, shutdown ordering, health, and public operator state. Never touches provider connections or sessions directly. |
| `cli` | `serve`, `config`, `doctor`, `profile`, `gh`, `status`, and migration/version surfaces. |

`runtime` 装配 `group` 与 provider 实现。`group` 和 `provider` 都依赖
core 的 `agent_session` 契约；adapter 不导入 Group、queue 或 store。
Group 使用 store/context/github/worktree 编排产品状态，不操纵连接 epoch。
Producer、queue、outbox 各自通过 store 收敛其负责的状态；只有需要平台
事实或写入的 producer/outbox 依赖 GitHub 网络操作。

### Internal Event Model

`store` owns the typed, platform-neutral event contract `EventKind`
(`assign`, `unassign`, `mention`, `wake`, `invalidate`, `lifecycle`,
`origin_echo`, `noop`) next to the events ledger it persists. A producer translates platform deliveries into
`EventKind` at ingress and records only the internal kind plus the per-platform
opaque Event Reference; `queue` and `group` consume `EventKind` exclusively
and never branch on platform event names or actions. This is the seam at
which a future non-GitHub platform plugs in: it adds a producer mapping, not
new consumer logic. The current GitHub mapping is owned by
[`github.md`](github.md).

### State authority

Every piece of state has exactly one authority; everything else is a
best-effort one-way projection from it. Nonessential state is not persisted.

| State | Authority | Projections |
| --- | --- | --- |
| Work Items, assignments, turns, context ledger/resets, queue/batches, outbox, owner lease | Durable store (SQLite) | In-memory `RunningAgentTurn` (claim cache for the in-flight turn), health snapshot |
| GitHub canonical state | GitHub | `canonical_objects` / `sync_cursors` snapshots for diffing |
| Physical session identity (`provider_session_id`) | Durable store (`provider_sessions`) | `SessionManager` 的 opaque key 与 adapter 寻址；句柄可替换，assignment 与 worktree 连续性不依赖连接 |
| Current provider turn | Provider process | `SessionEvent` stream (exactly one `TurnStarted`/`TurnTerminal` per turn; receiver handed off with the turn, never re-subscribed) → durable store; 恢复时 fencing 遗留的 starting/running turn |
| Provider connectivity | Provider connection | adapter 内部观察连接退出 → 受影响句柄的 latched availability；runtime 汇总各 driver 的恢复结果，成功不能覆盖另一方的失败 |
| Worktree presence | Filesystem + git | `worktrees` table (refreshed by inspection at prepare time) |

Selected dependency baseline, verified against crates.io on 2026-08-13:

- Tokio 1.53 for async runtime/process/I/O/signal ownership;
- Axum 0.8 for loopback HTTP and Reqwest 0.13 with rustls for GitHub/OTLP HTTP;
- Serde 1.0 and serde_json 1.0 for external JSON boundaries;
- rusqlite 0.40 with bundled SQLite, used only by a dedicated blocking DB
  actor so network awaits never occur inside transactions;
- Clap 4.6 (with `env` feature), `serde_path_to_error` for TOML error paths,
  `thiserror` for boundary errors, and `anyhow` only at CLI/runtime
  composition boundaries;
- HMAC 0.13/SHA-2 0.11 for webhook verification and context revisions,
  `jsonwebtoken` 11 for GitHub App JWTs;
- Comrak 0.54 to identify actual Markdown HTML comment nodes while preserving
  code literals;
- OpenTelemetry/SDK/OTLP 0.32 plus tracing-opentelemetry 0.33;
- UUID 1.24, `time` 0.3, and `url` for typed identities and timestamps.

Versions become lower/upper compatible ranges in `Cargo.toml`; `Cargo.lock` is
committed for the released binary. Rust 1.93 is the first verified toolchain.

## Cross-Unit Invariants

1. Every provider turn belongs to exactly one Work Item, Profile,
   Assignment Generation, Context Revision, and physical Provider Session.
2. A group admits one active turn in MVP. A stale Context Revision cannot
   publish Braid-owned writes after fencing.
3. Every turn uses a freshly materialized complete Context. Required pagination
   or hard-budget failure blocks before provider input.
4. A Provider Session is reusable only while Context schema/revision and
   effective instruction revision remain compatible. Hard Invalidation creates
   a fresh physical session in Codex v1.
5. Event References contain no copied GitHub prose. They identify what changed
   and where; current Context supplies durable content and `gh` supplies detail.
6. Braid App writes carrying durable operation correlation and direct writes
   from the Profile's explicitly configured stable GitHub actor are Agent-origin
   for self-wake/reset suppression. Other direct `gh` writes remain allowed but
   are processed as external activity.
7. Braid never mirrors turn output. Its only unsolicited comments are mutable
   Operational Status Comments.
8. Ordinary batches have no active/terminal reactions. Those states apply only
   to the exact trusted `@braid` comment that started the turn.
9. GitHub/network I/O never occurs in a SQLite transaction. Every Braid-owned
   mutation has a durable intent before leaving the process.
10. Provider terminal means only that a turn ended; it never proves task,
    implementation, review, or acceptance success.

## Durable State and Migrations

SQLite starts in WAL mode with foreign keys, a busy timeout, and FULL
synchronous durability. One DB actor serializes writes. Schema v1 owns:

- `schema_migrations(version, name, checksum, applied_at)`;
- `owner_leases(scope, generation, owner_id, expires_at)`;
- `repositories` and `work_items` keyed by stable GitHub node IDs;
- `profiles` with immutable revision and effective-config digest;
- `assignments` and `agent_instances` with group/profile/generation/lifecycle;
- `provider_sessions` and `turns` with opaque provider IDs and Context revision;
- `worktrees` for the default PR Implementation Agent workspace;
- `associations` for direct Issue↔PR edges and their observed version;
- `issue_context_sources` for one exact visible-description comparison state
  per Issue, independent of association fan-out;
- `canonical_objects` for latest object version/lifecycle, the internal
  reconciliation equality fingerprint, and deletion tombstones;
- `deliveries` and `events` for webhook/reconciliation dedupe/classification;
- `scheduler_batches` and `batch_events` for quiet/count/urgent state;
- `write_intents`, `reaction_targets`, and `status_comments` for Braid-owned
  GitHub convergence;
- `sync_cursors` for repository reconciliation.

Migration files are embedded, monotonically numbered, forward-only, and never
edited after release. Startup takes an exclusive migration lease, verifies all
previous checksums, applies each migration in one transaction, and rejects a DB
newer than the binary. Compatible application rollback is declared per release;
an incompatible schema rollback restores the pre-migration backup rather than
running a down migration.

For any Agent-serving Profile, `workspace` names the instance source Git
checkout of the configured repository (one repository = one source checkout,
defaulting to `<instance>/source`), not the directory in which the Agent
edits. Every Agent Group session runs in a dedicated generation-scoped
worktree that Braid provisions from that checkout under the configurable
`[runtime] worktrees` directory (default `<state>/worktrees`; only new
generations use a changed location — SQLite records each worktree's actual
path):

- PR Agent Group: `runtime.root/worktrees/pr-<number>/<profile>-g<generation>`,
  bound to the fetched PR head;
- Issue Agent Group: `runtime.root/worktrees/issue-<number>/<profile>-g<generation>`,
  bound to the Issue's sole same-repository Development linked branch when
  exactly one exists, otherwise to the repository default branch
  (`refs/remotes/origin/<default>`). Several Development branches are not an
  error: the worktree still starts on the default branch, and the Braid
  System Prompt tells the Agent it may switch or create branches in its
  worktree as the work requires (Context lists the Development branches).

Every worktree gets `.braid/` added to its `.git/info/exclude` at provision
time: that directory persists across Provider Session replacement within the
same assignment generation and stays out of `git status`, commits, and
GitHub. The System Prompt states these facts and nothing more — whether and
how the Agent uses the directory is its own choice.

SQLite records the resolved source, worktree, bound ref, and local branch as
operational facts. The provider session is started and later resumed only
against that worktree. This provides isolation and recovery identity without
turning Braid into a Git policy engine.

## Error and Concurrency Model

External shapes deserialize into typed structures with explicit unknown
variants; unknown methods/actions/unions become bounded `unsupported` evidence
rather than generic JSON forwarded to the Agent. Boundary errors carry a stable
category plus source error; Human-facing status never exposes Rust backtraces as
Agent prose.

The runtime has one repository owner lease and per-Agent-Group single-flight
turn ownership. Lease generation fences stopped processes. Braid-owned creates
and updates use the write-outbox state machine; direct Agent `gh`/`git` actions
are outside it and converge only when GitHub reports them.

## Realization Pointers

- Exact Context and revision: [`context.md`](context.md)
- Event, group, session, and reaction state machines: [`lifecycle.md`](lifecycle.md)
- Codex mapping and provider seam: [`app-server.md`](app-server.md)
- GitHub ingress, canonical reads, associations, identity, and writes:
  [`github.md`](github.md)
- Packaging, OTel, tunnel, migration, and operation:
  [`../40-deployment/README.md`](../40-deployment/README.md)
- External acceptance oracle: [`../10-prd/acceptance.md`](../10-prd/acceptance.md)

## Console 与控制边界

Console 的 OTLP Collector 保存原始传输批次与完整解码记录，使用独立数据库，
不能写回对象或执行权威。资源身份、runtime generation、session、turn 与原生
协议身份显式关联；采样和未关联记录是证据缺口，不是空闲状态。Web 查询分页
读取该读模型，当前对象来自协作平台 API，执行状态来自 Harness ledger 与 runtime manager。

平台通过有类型的应用命令修改对象。命令 request ID、内容摘要、预期 revision
与回执持久保存在平台库；普通对象变更与事件、回执一起提交，
并发正文修改不会覆盖新 revision。发布和合并保留 Git/SQLite 意图边界，不能用
外层数据库事务假装跨系统原子化。浏览器只能选择固定 source 或由 Harness 验证的候选，不能提供任意路径。

`console::runtime` 持有实际 runtime task、停止通道与启动 generation。Collector
、平台和控制端点持续存活，Harness 可单独停止。GitHub 与内建上游通过统一 runtime 接口托管。请求接受不等于任务已就绪或已停止；
监督器等待所有 Group/provider 停止结果，失败进入 blocked 并禁止新执行覆盖。
Console 不调用 CLI，也不复制 Factory 的实验、赛事或容器生命周期。
