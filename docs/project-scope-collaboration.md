# Project-scope Collaboration Instructions

The `GitHub-bound Coding Tasks` section is installed in this project's
`AGENTS.md`. Codex therefore loads it for work under `agent-handoff/` without
changing ordinary chat or work in other projects.

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
- Raw chain-of-thought is excluded, while reasoning summaries and protocol
  activity may be present in GitHub's raw comment body and must not contain
  credentials or private data.

The complete authoritative wording lives in `../AGENTS.md`; this page explains
its scope and should not become a second editable copy.
