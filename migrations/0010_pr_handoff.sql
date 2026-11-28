-- First acceptance freezes the existing target generation, never an activation.
CREATE TABLE pr_handoffs (
    intent_id TEXT PRIMARY KEY REFERENCES write_intents(intent_id),
    target_assignment_id TEXT NOT NULL REFERENCES assignments(assignment_id),
    event_id TEXT UNIQUE REFERENCES events(event_id)
) STRICT;
