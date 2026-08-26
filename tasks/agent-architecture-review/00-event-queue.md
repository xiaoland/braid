# Event Queue and Event Producer

## Goal

Define the boundary between the GitHub-facing **Event Producer**, the per-work-item
per-agent-group **Event Queue**, and the **Agent Group**.

## Agreed Direction

- **Event Producer** sits behind GitHub:
  - consumes webhooks and/or GraphQL reconciliation;
  - normalizes raw GitHub activity into Braid domain events (comment created,
    title edited, review submitted, etc.);
  - classifies events by semantic type: `Wake`, `HardInvalidation`,
    `CrossSurfaceInvalidation`, `Lifecycle`, `UrgentMention`, `AgentOrigin`.
- **Event Queue** is **per work-item per agent-group** and owns the quiet window
  / threshold. When the window closes or the threshold is hit, the queue emits
  a batch (a plain user message plus optional new context and steering flag) to
  its Agent Group.
- **Agent Group** is a thin manager that sends user messages to the Agent
  Session. It does **not** manage session lifecycle, inspect `status()`, or
  decide when to create sessions. It receives user message text, optional new
  context, and steering flag, and forwards them via `send_user_msg(...)`.
- The Agent Session internally handles queuing, steering, and context-replacement
  decisions. The caller does not branch on `status()` before calling
  `send_user_msg`.
- Events carry:
  - `source`: the work item / surface they came from (used to match
    `profile.handlers_of`).
  - semantic `type` (see above).
- `ordinary|interrupt` is a scheduler action, not an event type.

## Decisions

1. **Routing model**: explicit routing. The Event Producer determines which
   (work-item, agent-group) queues need an event and pushes it directly. This
   avoids a shared bus and keeps queue ownership simple for MVP.
2. **Session provisioning**: the Event Producer / its wrapper creates the Agent
   Session at Issue/PR activation time and passes the session handle to the
   Agent Group. The Event Queue does not know about sessions; it only emits
   batches to the Agent Group.
3. **AgentOrigin events**: dropped at the producer for the originating group
   (no wake / no reset). They may still update the local canonical ledger.
   Other groups see them as ordinary external events.
4. **Hard Invalidation**: the Event Queue turns the latest Context into the
   optional new context and passes it to the Agent Group with the batch. The
   Agent Group does not know about invalidation semantics; it just forwards
   `reset_context_to` if present.

## MVP Simplification

- One Agent Session per Agent Group.
- Parallel symmetric fan-out is an architectural placeholder, not implemented
  now.
