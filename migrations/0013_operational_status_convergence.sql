-- Status remains one stable comment per work item/Profile/generation.
-- The outbox mapping survives replacement of a desired publication while an
-- older create is in flight, so its remote identity is never orphaned.
ALTER TABLE status_comments ADD COLUMN projection_kind TEXT NOT NULL DEFAULT 'failure'
    CHECK (projection_kind IN ('failure','recovered','superseded'));
ALTER TABLE status_comments ADD COLUMN failure_body TEXT;
ALTER TABLE status_comments ADD COLUMN failure_recorded_at TEXT;
ALTER TABLE status_comments ADD COLUMN successor_generation INTEGER;
UPDATE status_comments SET
    failure_body=(SELECT content FROM github_write_outbox o WHERE o.intent_id=status_comments.write_intent_id),
    failure_recorded_at=COALESCE((SELECT created_at FROM github_write_outbox o WHERE o.intent_id=status_comments.write_intent_id),updated_at);
ALTER TABLE github_write_outbox ADD COLUMN status_work_item_node_id TEXT;
ALTER TABLE github_write_outbox ADD COLUMN status_profile_id TEXT;
ALTER TABLE github_write_outbox ADD COLUMN status_generation INTEGER;
UPDATE github_write_outbox SET
    status_work_item_node_id=(SELECT work_item_node_id FROM status_comments s WHERE s.write_intent_id=github_write_outbox.intent_id),
    status_profile_id=(SELECT profile_id FROM status_comments s WHERE s.write_intent_id=github_write_outbox.intent_id),
    status_generation=(SELECT assignment_generation FROM status_comments s WHERE s.write_intent_id=github_write_outbox.intent_id)
WHERE EXISTS(SELECT 1 FROM status_comments s WHERE s.write_intent_id=github_write_outbox.intent_id);
CREATE INDEX outbox_status_publications ON github_write_outbox(status_work_item_node_id,status_profile_id,status_generation,lifecycle);
