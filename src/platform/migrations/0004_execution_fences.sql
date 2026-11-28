CREATE TABLE execution_fences (
    client_id TEXT NOT NULL,
    execution_id TEXT NOT NULL,
    PRIMARY KEY (client_id, execution_id)
) STRICT;
ALTER TABLE platform_grants ADD COLUMN execution_id TEXT;
-- Old tokens cannot be attributed to an execution attempt. Preserve their
-- epoch and history while retiring their permission.
UPDATE platform_grants SET lifecycle='revoked' WHERE execution_id IS NULL;
