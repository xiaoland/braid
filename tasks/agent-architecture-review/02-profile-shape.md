# Agent Profile Shape

## Goal

Decide what belongs in Agent Profile and what belongs in referenced registries.

## Agreed Direction

- `braid gh` is exposed as a **command-line tool in the Agent runtime's shell
  environment**, not as a tool-use / JSON-RPC call. The Agent invokes it via
  shell commands.
- Agent Profile is a **role snapshot**, not a provider connection spec.
- Fields:
  - `id`, `display_name`, `priority`
  - `scopes` (`issue|pr`) replacing current `tags`
  - `user_instructions`
  - `adapter` reference (e.g., `"codex-app-server"`)
  - `provider` + `model` reference
  - `reasoning`, `sandbox` policy, `workspace`
  - `skills` and `mcps` as **references to a global registry**
- Sub-agent config is **adapter/skill-specific**, not inline in the generic
  profile.
- LLM cost/allowance is **not** in profile.

## Decisions

1. `github_actor_node_id` and `status_surfaces` stay in the Agent Profile. Both
   are product-defined profile behavior (glossary: Agent Attribution /
   Operational Status Comment), not implementation detail.
2. Adapter version pin lives in the runtime registry; the profile references
   the adapter id only.
3. MVP TOML layout:

```toml
[[profiles]]
id = "default"
display_name = "Default"
priority = 1
scopes = ["issue", "pr"]
user_instructions = "..."
adapter = "pi"
provider = "openai-codex"
model = "gpt-5.2-codex"
reasoning = "high"
sandbox = "workspace-write"
workspace = "agent"
github_actor_node_id = "..."
status_surfaces = ["issue_body", "pr_body"]
skills = ["gh"]
mcps = ["time"]
```

## Pending Decision

None; finalize the implementation layout when the new schema lands.
