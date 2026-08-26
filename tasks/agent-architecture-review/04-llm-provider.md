# LLM Provider / Cost / Allowance

## Goal

Separate model service configuration from Agent Profiles.

## Agreed Direction

- `llm_providers` table is global per config.
- Each provider entry has:
  - `id`, protocol, connection config, API key reference
  - `models` list with `model_id`, `input_cost`, `output_cost`, `cache_input_cost`
    (per-million-token USD)
  - `allowances` with `since`, `until`, `amount` (USD), optional `profile_ids`
- Profiles only reference provider + model.

## Decisions

1. Allowance enforcement is **not** implemented in this architecture pass. The
   `llm_providers` schema includes `allowances` as metadata only; runtime
   enforcement (budget checks, hard stops) is deferred until we have usage
   telemetry and a real cost model.
2. The provider API key lives in the per-owner secrets file (`braid-of-<owner>.secrets.toml`)
   and is referenced by `api_key_file` in the `llm_providers` entry. This matches
   the current multi-owner setup design.

## Pending Decision

None; implement schema and secret loading, then wire into profile resolution.
