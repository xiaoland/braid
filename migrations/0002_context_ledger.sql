ALTER TABLE canonical_objects ADD COLUMN database_id TEXT;
ALTER TABLE canonical_objects ADD COLUMN author_login TEXT;
ALTER TABLE canonical_objects ADD COLUMN reference_repository TEXT;
ALTER TABLE canonical_objects ADD COLUMN reference_number INTEGER;
ALTER TABLE canonical_objects ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1));

CREATE INDEX canonical_objects_lifecycle
ON canonical_objects(work_item_node_id, object_kind, lifecycle);
