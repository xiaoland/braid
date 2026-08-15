CREATE TABLE context_resets (
    reset_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agent_instances(agent_id),
    old_session_id TEXT NOT NULL REFERENCES provider_sessions(session_id),
    active_turn_id TEXT REFERENCES turns(turn_id),
    new_session_id TEXT REFERENCES provider_sessions(session_id),
    context_revision_before TEXT NOT NULL,
    context_revision_after TEXT,
    continuation INTEGER NOT NULL CHECK (continuation IN (0, 1)),
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN ('interrupting', 'materializing', 'applied', 'blocked')
    ),
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX context_resets_active_agent
ON context_resets(agent_id)
WHERE lifecycle IN ('interrupting', 'materializing');

CREATE TABLE context_reset_events (
    reset_id TEXT NOT NULL REFERENCES context_resets(reset_id),
    event_id TEXT PRIMARY KEY REFERENCES events(event_id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0)
) STRICT;
