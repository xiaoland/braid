ALTER TABLE turns ADD COLUMN batch_id TEXT REFERENCES wake_batches(batch_id);

CREATE UNIQUE INDEX turns_batch
ON turns(batch_id)
WHERE batch_id IS NOT NULL;

CREATE UNIQUE INDEX assignments_active_work_item
ON assignments(work_item_node_id)
WHERE lifecycle IN ('materializing', 'active');

CREATE UNIQUE INDEX github_write_outbox_request
ON github_write_outbox(request_digest);

CREATE INDEX provider_sessions_lifecycle
ON provider_sessions(lifecycle, started_at);

CREATE INDEX turns_lifecycle
ON turns(lifecycle, started_at);
