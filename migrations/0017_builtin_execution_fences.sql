ALTER TABLE provider_sessions ADD COLUMN builtin_execution_id TEXT;
ALTER TABLE provider_sessions ADD COLUMN builtin_execution_owner TEXT;
ALTER TABLE provider_sessions ADD COLUMN builtin_execution_state TEXT NOT NULL DEFAULT 'open'
    CHECK (builtin_execution_state IN ('open','fencing','fenced'));
CREATE UNIQUE INDEX provider_builtin_execution ON provider_sessions(builtin_execution_id)
    WHERE builtin_execution_id IS NOT NULL;
ALTER TABLE turns ADD COLUMN builtin_execution_id TEXT;
ALTER TABLE builtin_writer_grants ADD COLUMN execution_id TEXT;
