# Event Queue and Event Producer

## Goal

Define the boundary between the GitHub-facing **Event Producer**, the per-work-item
per-profile **Event Queue**, and the **Agent Group**. Module names in code follow
the TDD (`events`, `scheduler`, `sessions`); the producer/queue/group terms are
the conceptual roles of those modules.

## Taxonomy (aligned with `lifecycle.md` and `store.events`)

- `classification` ∈ `wake | hard_invalidation | cross_surface_invalidation |
  dependency_dirty | lifecycle` (+ none for non-semantic changes).
- `origin` ∈ `agent | external` — a separate field, not a classification.
- A Trusted Braid Mention is an **urgent property** of a `wake` event, not a
  type; it bypasses quiet window and count.

## Agreed Direction

- **Event Producer** (`github` ingress + `events` classification):
  - consumes webhooks and GraphQL reconciliation;
  - diffs against the canonical ledger and emits classified events with
    origin attribution (agent-origin writes are attributed via durable
    operation correlation or the profile's configured stable actor node id);
  - agent-origin events update the ledger but never wake/reset the
    originating group; other groups see them as external.
- **Event Queue** (`store.scheduler_batches` + `scheduler`): per work-item per
  profile; owns quiet window (30s default) and count threshold (8 default),
  both profile-overridable; one pending batch per group; emits
  (user message text, optional new context, steering) to the Agent Group.
- **Agent Group** (`sessions`): thin forwarder. It does not manage session
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
3. **Hard Invalidation / Dependency Dirty**: on batch emission, `sessions`
   materializes the current Context; if its revision advanced, the batch is
   sent with `reset_context_to=Some(context)`. The Agent Group does not know
   invalidation semantics.
4. **Transport unknown**: disconnect is not terminal; the adapter
   reconnects/resumes internally and the core keeps seeing `running` until
   the adapter proves failure.

## MVP Simplification

- One Agent Session per Agent Group.
- Parallel symmetric fan-out is an architectural placeholder, not implemented
  now.
