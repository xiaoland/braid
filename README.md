<p align="center">
  <img src="docs/assets/braid-logo-128.png" alt="Braid logo" width="96" height="96">
</p>

<h1 align="center">Braid</h1>

<p align="center">
  <strong>Context that follows the work, not the chat history.</strong>
</p>

<p align="center">
  Braid gives local coding agents a different way to work on long-running software tasks:<br>
  build their context from the <em>current</em> state of GitHub Issues and pull requests,<br>
  instead of treating an ever-growing conversation as the project's memory.
</p>

<p align="center">
  <a href="#why-braid">Why Braid</a> ·
  <a href="#how-it-works">How it works</a> ·
  <a href="#get-started">Get started</a> ·
  <a href="#documentation">Documentation</a>
</p>

## Why Braid

A software task does not evolve like a chat transcript. Requirements are rewritten. Decisions are reversed. Reviews are resolved. Work continues across days, people, and agent sessions.

A conventional, append-only agent conversation can retain both an old decision and the correction that superseded it. Compacting that transcript may reduce its size, but compaction alone does not establish which GitHub facts are currently authoritative.

**Braid makes the current collaboration state the starting point for context.**

![A comparison of append-only chat history and Braid's context rebuilt from current GitHub state](docs/assets/context-model.svg)

When an Issue changes from decision **A** to **B**, a new Braid context reflects the current Issue description and visible discussion. Material changes to facts already in an agent's context can invalidate the old provider session, so subsequent work need not carry the superseded text forward.

GitHub retains its normal history; **Braid changes what enters the agent's working context**.

## How it works

Braid connects a local coding agent to the collaboration surfaces developers already use:

1. **Discuss in Issues.** An Issue holds the current problem, design, relevant metadata, and visible discussion. An Issue agent can participate and maintain that shared working state.
2. **Implement in pull requests.** An implementation request can create a draft PR and a dedicated Git worktree. The PR agent works from the current PR and its associated Issues, rather than a frozen copy of the original request.
3. **Rebuild when the work changes.** GitHub events bring changes to Braid's attention. Braid reads canonical GitHub state to build context; material edits can trigger session replacement while the task's code workspace remains available.

The model has three deliberately different parts:

| | Responsibility |
| --- | --- |
| **GitHub** | Shared, editable working memory: current requirements, decisions, discussion, and relationships. |
| **Git worktree** | Durable task-local code and working files, preserved across compatible provider-session replacements. |
| **Provider session** | Replaceable execution context used by the coding agent, not the authority for the task. |

Braid is a local Rust application. It uses a GitHub App for the collaboration boundary, and supports coding-agent providers through runtime adapters. It does not require a hosted Braid service.

## Get started

The current packaged installation targets **macOS on Apple Silicon**. You'll need authenticated [GitHub CLI (`gh`)](https://cli.github.com/), a repository where you can install a GitHub App, and credentials for your selected coding-agent provider. See the [setup guide](docs/user-manual/setup.md) for full prerequisites and Codex/Pi configuration.

**1. Install Braid**

```sh
brew install xiaoland/braid/braid
```

**2. Set up your GitHub App and agent**

For example, using Pi with a DeepSeek API key available as `DEEPSEEK_API_KEY`:

```sh
braid setup owner/repository --provider pi --model deepseek-chat --api-key-environment DEEPSEEK_API_KEY
```

Follow the printed GitHub App installation link, then check the instance and start the local runtime:

```sh
braid doctor --instance owner
braid serve --instance owner --tunnel
```

The instance key defaults to the repository owner; replace `owner` with your chosen key if you configured one. `--tunnel` exposes the local webhook endpoint through a supervised temporary tunnel.

**3. Start from a GitHub Issue**

A repository maintainer can activate Braid by posting a visible, trusted `@braid` mention in an Issue. Ordinary GitHub Apps cannot be selected as native Issue assignees; assignment requires a separately provisioned GitHub Agent App. The [setup guide](docs/user-manual/setup.md#activating-braid-on-an-issue) explains this distinction.

> [!NOTE]
> Braid is an early-stage, local-first project. Review the [supported scope](docs/10-prd/scope.md) and [deployment guide](docs/40-deployment/README.md) before relying on it for unattended work.

## Design boundaries

- **Current state over transcript history.** Braid deterministically projects the GitHub objects that matter to a task; it does not ask another model to invent a replacement summary.
- **Events are signals, not truth.** A webhook tells Braid that something changed. The authoritative content comes from GitHub's current state.
- **Changes can remove stale context.** Editing or removing material already projected into a session can cause Braid to rebuild context and replace that session.
- **Agents can update their own working memory.** Useful design decisions belong in Issue/PR descriptions and concise discussion; every internal turn need not become a public comment.
- **Incomplete context is visible.** When the complete context cannot be made available, Braid does not silently truncate it and send an apparently complete view to the agent.

## Documentation

| Start here | For |
| --- | --- |
| [Setup guide](docs/user-manual/setup.md) | GitHub App bootstrap, provider credentials, activation, and local runtime. |
| [Product model](docs/10-prd/README.md) | Purpose, working memory, Issue/PR collaboration, and current product scope. |
| [Context design](docs/20-product-tdd/context.md) | Current-state projection, revisions, invalidation, and replacement. |
| [Runtime lifecycle](docs/20-product-tdd/lifecycle.md) | Events, scheduling, sessions, and recoverability. |
| [Deployment & observability](docs/40-deployment/README.md) | Configuration, migrations, diagnostics, and operations. |
| [Real end-to-end acceptance](docs/10-prd/acceptance.md) | How the complete GitHub-to-agent workflow is verified. |

For source development, Braid is a single Rust package that produces a portable `braid` executable. Build with `cargo build --locked`; the repository's product and technical contracts live in [`docs/`](docs/10-prd/README.md). See the [MIT license](LICENSE).

