# Agent Architecture Review

Split into focused sub-packets so each boundary can be closed independently.

- **00-event-queue.md**: event model, sources, and scheduling semantics.
- **01-session-interface.md**: `AgentSession` contract, `send_user_msg`,
  `reset_context_to`, and who owns session identity/forking.
- **02-profile-shape.md**: what belongs in Agent Profile vs. runtime registry vs.
  LLM provider vs. skill/MCP registry.
- **03-runtime-registry.md**: adapter/runtime executable metadata, version pins,
  download/isolation.
- **04-llm-provider.md**: LLM service config, models, cost, allowance.
- **05-worker-layout.md**: per-worker folder consolidating config, secrets, db,
  runtimes, worktrees, logs.
- **06-implementation-plan.md**: incremental PR sequence.

**Current status**: all packets are decided; implementation plan drafted and
ready for review.
