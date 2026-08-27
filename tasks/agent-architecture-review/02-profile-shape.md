# Agent Profile Shape

## Goal

Decide what belongs in Agent Profile and what belongs in referenced registries.

## Agreed Direction

- `braid gh` is exposed as a **command-line tool in the Agent runtime's shell
  environment**, not as a tool-use / JSON-RPC call.
- Agent Profile is a **versioned, immutable role snapshot** (store:
  `profiles` with immutable revision + effective-config digest), not a
  provider connection spec.
- Fields:
  - `id`, `display_name`, `priority`
  - `tags` (`issue`/`pr`) — product vocabulary (glossary: Profile Tag); **not**
    renamed to `scopes`
  - `user_instructions`
  - `adapter_type` + `adapter_version` (locates an Agent Runtime Adapter
    class)
  - `provider` + `model` (resolves into `llm_providers`)
  - `reasoning`, `sandbox` policy, `workspace`
  - `skills` and `mcps` as names resolved by the adapter against the runtime
    home
- Profile **never** carries connectivity config (`executable_path`, `api_url`,
  `CODEX_HOME`/`PI_HOME`-style homes): profile user_instructions/skills/mcps
  are implemented against one fixed runtime home, so the home lives in the
  worker-level runtime registry entry.
- Sub-agent config is adapter/skill-specific, not inline in the profile.
- LLM cost/allowance is not in the profile.

## Decisions

1. `github_actor_node_id` and `status_surfaces` stay in the Agent Profile
   (glossary: Agent Attribution / Operational Status Comment).
2. Profile revision/digest participates in session compatibility checks
   (TDD invariant 4): a changed profile revision forces fresh session
   materialization.
3. MVP TOML layout:

```toml
[[runtimes]]                  # one entry per adapter_type per worker
adapter_type = "pi"
version = "0.84.3"            # verified at setup time
executable_path = "..."       # adapter-defined connectivity config
home = "..."                  # PI_HOME lives here, not in any profile

[[profiles]]
id = "default"
display_name = "Default"
priority = 1
tags = ["issue", "pr"]
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

None.
