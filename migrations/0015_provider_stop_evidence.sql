-- Only a confirmed native close may record a stop. Inherited session state
-- does not prove execution stopped, including idle and reset_pending.
ALTER TABLE provider_sessions ADD COLUMN execution_stopped_at TEXT;
