# Project-scope Collaboration Instructions

The `GitHub-bound Coding Tasks` section is installed in this project's root
`AGENTS.md`. Codex therefore loads it for work in Braid without changing
ordinary chat or work in other projects.

The section is the durable collaboration contract for this PoC:

- GitHub comments remain messages rather than implicit commands.
- The Wrapper only delivers GitHub references and mirrors Agent turns; it does
  not interpret readiness, manage worktrees, or perform Agent-owned GitHub
  actions.
- The bound Issue is the design and acceptance source of truth; an associated
  Draft PR is one candidate implementation.
- Every bound implementation uses its supplied dedicated worktree and branch.
- Exact visible trusted `@agent` is only a scheduling hint that skips settling.
- Context persistence, compaction, and resume remain provider-owned.
- Raw chain-of-thought and arbitrary protocol events are excluded. Braid
  publishes assistant messages, provider-labelled reasoning summaries, and
  schema-mapped tool calls as visible Markdown. Tool calls use a short Human
  summary plus bounded call/result evidence inside `<details>`; process and
  thread IDs, whole environments, credentials, binary data, hidden debug
  payloads, and ownership markers are never published.

The complete authoritative wording lives in `../AGENTS.md`; this page explains
its scope and should not become a second editable copy.
