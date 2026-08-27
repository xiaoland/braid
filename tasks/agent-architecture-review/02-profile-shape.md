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
  - `adapter_type` + `adapter_version` reference (locates an Agent Runtime
    Adapter class; connectivity config lives in the worker-level runtime
    registry, never in the profile — including `CODEX_HOME`/`PI_HOME`-style
    homes, because profile user_instructions/skills/mcps are implemented
    against a fixed runtime home)
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
2. Adapter contract version pin: profile carries `adapter_type` +
   `adapter_version`; the worker-level registry entry holds the connectivity
   config for that adapter class.
3. MVP TOML layout:

```toml
[[runtimes]]
adapter_type = "pi"
version = "0.84.3"
executable_path = "..."     # adapter-defined connectivity config
home = "..."                # PI_HOME lives here, not in any profile

[[profiles]]
id = "default"
display_name = "Default"
priority = 1
scopes = ["issue", "pr"]
user_instructions = "..."
adapter_type = "pi"
adapter_version = "0.84.3"
provider = "deepseek"
model = "deepseek-v4-pro"
reasoning = "high"
sandbox = "workspace-write"
workspace = "agent"
github_actor_node_id = "..."
status_surfaces = ["issue", "pr"]
skills = ["gh"]
mcps = ["time"]
```

## Pending Decision

None; finalize the implementation layout when the new schema lands.
