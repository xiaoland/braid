# Event Queue and Event Producer

## Goal

Define the boundary between the GitHub-facing **Event Producer**, the per-work-item
per-profile **Event Queue**, and the **Agent Group**. These three are the new
architecture's own modules (`producer`, `queue`, `group`); the old TDD module
table is being replaced, not mapped onto.

## Taxonomy (aligned with `lifecycle.md` and `store.events`)

- `classification` ∈ `wake | hard_invalidation | cross_surface_invalidation |
  dependency_dirty | lifecycle` (+ none for non-semantic changes).
- `origin` ∈ `agent | external` — a separate field, not a classification.
- A Trusted Braid Mention is an **urgent property** of a `wake` event, not a
  type; it bypasses quiet window and count.

## Agreed Direction

- **Event Producer** (`producer`): webhook/GraphQL ingress, canonical
  diff classification, origin attribution, explicit routing.
- **Event Queue** (`queue`): per work-item per profile; quiet window (30s) and
  count threshold (8), profile-overridable; one pending batch per group; emits
  (user message text, latest Context, steering) to the Agent Group.
- **Agent Group** (`group`): thin forwarder. It does not manage session
  lifecycle, inspect `status()` before sending, or create sessions; the
  producer/activation path creates the session and hands it over. It calls
  `send_user_msg(...)` and consumes `SessionEvent`s for reactions and the
  group state machine.
- `ordinary|interrupt` is a scheduler action, not an event type.

## Decisions

1. **Routing**: explicit — the producer writes each event into the queues of
   the (work-item, profile) pairs it affects (cross-surface fan-out for
   direct Associated Issues per lifecycle.md). No shared bus.
2. **Session provisioning**: at activation (Issue assignment / mention
   fallback / PR activation), the producer-side wrapper asks `sessions` to
   materialize Context + create the physical session via the adapter; the
   resulting logical `AgentSession` handle is stored with the group.
3. **Context freshness**: on batch emission, the group-side materializes the
   current Context from GitHub and passes it as `reset_context_to`; the adapter
   internally decides whether a physical reset is needed (no revision model).
4. **Transport unknown**: disconnect is not terminal; the adapter
   reconnects/resumes internally and the core keeps seeing `running` until
   the adapter proves failure.

## MVP Simplification

- One Agent Session per Agent Group.
- Parallel symmetric fan-out is an architectural placeholder, not implemented
  now.
