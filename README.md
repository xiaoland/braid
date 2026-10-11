<p align="center">
  <img src="docs/assets/braid-logo-128.png" width="128" height="128" alt="Braid logo">
</p>

# Braid

Braid turns Issues and pull requests into durable working memory for
local Coding Agents. GitHub or a Braid-owned local space holds the current design, implementation state,
metadata, relationships, and discussion; Braid rebuilds a provider session from
that state whenever prior context becomes stale.

Braid is implemented as one Rust package and one portable `braid` binary.

## Install

On macOS arm64:

```shell
brew install xiaoland/braid/braid
```

## 内建协作空间

Braid 可以直接持有 Issue、讨论、指派和 PR，无需 GitHub App。每个实例选择
一个对象权威：GitHub 或本地 SQLite。原生 Agent CLI 和模型服务仍是执行依赖。

```shell
braid init --source /absolute/path/to/repository --config /absolute/path/to/space/config.toml --name demo --adapter pi --executable /absolute/path/to/pi --provider deepseek --model deepseek-chat --api-key-environment DEEPSEEK_API_KEY
braid migrate apply --config /absolute/path/to/space/config.toml
braid local --config /absolute/path/to/space/config.toml issue --title '需要完成的工作' --body '目标与依据'
braid serve --config /absolute/path/to/space/config.toml
```

对象操作、候选发布和合并见[本地协作指南](docs/user-manual/local.md)。

## Bootstrap

`braid setup` creates your own GitHub App, persists credentials outside the
repository, and writes a starter instance config. The full setup guide is in
[`docs/user-manual/setup.md`](docs/user-manual/setup.md).

```shell
braid setup owner/repository --provider pi --model deepseek-chat --api-key-environment DEEPSEEK_API_KEY
```

The command opens a browser for GitHub's App Manifest flow, receives the
redirect locally, and saves the App private key and webhook secret under
`~/.braid/instances/<key>/` (the instance key defaults to the repository
owner). After the browser installs the App on your repository, run Braid
against that instance:

```shell
braid doctor --instance <KEY>
braid serve --instance <KEY> --tunnel
```

The active product and implementation contracts are:

- [Product Truth](docs/10-prd/README.md)
- [Real end-to-end acceptance](docs/10-prd/acceptance.md)
- [Rust Product TDD](docs/20-product-tdd/README.md)
- [GitHub Context](docs/20-product-tdd/context.md)
- [Event/session lifecycle](docs/20-product-tdd/lifecycle.md)
- [Codex provider contract](docs/20-product-tdd/app-server.md)
- [GitHub boundary](docs/20-product-tdd/github.md)
- [Deployment and observability](docs/40-deployment/README.md)
- [Glossary](docs/10-prd/glossary.md)

The first supported delivery target is a packaged macOS arm64 binary; Linux
x86_64 follows. Build and inspect the public operator surface with:

```shell
cargo build --locked
cargo run --locked -- --version
cargo run --locked -- config check --config /absolute/path/to/config.toml
cargo run --locked -- migrate plan --config /absolute/path/to/config.toml
cargo run --locked -- github probe --config /absolute/path/to/config.toml --repository owner/repository
cargo run --locked -- context issue owner/repository#123 --config /absolute/path/to/config.toml
cargo run --locked -- gh comment create owner/repository#123 --config /absolute/path/to/config.toml --profile issue-codex --body 'Concise update'
cargo run --locked -- gh pr ensure --comment 123456789 --config /absolute/path/to/config.toml
cargo run --locked -- serve --config /absolute/path/to/config.toml --tunnel
cargo run --locked -- serve --config /absolute/path/to/config.toml --transport-only
cargo run --locked -- status --config /absolute/path/to/config.toml --json
```

Apply all pending migrations before `context`; the local canonical ledger keeps
only mechanical versions, associations, and deleted-comment tombstones while
GitHub remains the content authority.
The diagnostic `--page-size` defaults to GitHub's maximum of 100; real campaign
helpers may lower it to force pagination while requiring byte-identical Context.
`serve` owns the configured Codex app-server and Issue Agent turns;
`--transport-only` deliberately stops at verified webhook ingress, canonical
reconciliation, reactions, and runnable debounce batches. `--tunnel` adds the
free supervised Quick Tunnel to either mode.
`braid gh` is the Braid-App-authored write surface. Comment creation prepends
the configured public Profile/role attribution and returns a durable receipt;
`pr ensure` uses the triggering Issue comment ID as its concurrency-safe
Implementation Request key.

Copy [`config.example.toml`](config.example.toml) outside the checkout and
replace every placeholder path before running diagnostics, or let `braid setup`
generate an instance config for you. A packaged release
does not require Python, PDM, Cargo, or a source checkout.

Braid 使用编译、实际操作及获授权实验取得反馈，不维护或运行测试代码、模拟提供方、
包 smoke 或自检入口。GitHub、provider 和 OTel 原始证据及人工判断构成产品验收依据。

## 本地 Web Console

内置 Console 接收 OTLP HTTP/protobuf，持久保存 traces、logs、metrics 的原始证据，
并提供浏览器可视化。连接 Local 配置时可直接管理对象与启停 runtime，不经 CLI
子进程。操作方法和数据边界见[Console 指南](docs/user-manual/console.md)。
