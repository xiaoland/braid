-- A projection read is not an acknowledgement of responsibility input.
-- Only a complete observation and its classified events advance this frontier.
CREATE TABLE processed_objects (
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    node_id TEXT NOT NULL,
    database_id TEXT,
    object_kind TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL CHECK (length(digest)=64),
    lifecycle TEXT NOT NULL,
    author_node_id TEXT,
    author_login TEXT,
    metadata_digest TEXT CHECK(metadata_digest IS NULL OR length(metadata_digest)=64),
    PRIMARY KEY(work_item_node_id,node_id)
) STRICT;

-- Establish a new observation epoch from retained current state. This does not
-- claim that historically missing inputs have been executed; old gaps require
-- explicit operator recovery, never automatic activation of a real task.
INSERT INTO processed_objects
SELECT c.work_item_node_id,c.node_id,c.database_id,c.object_kind,c.version,c.digest,
       CASE WHEN c.object_kind IN ('issue','pr') THEN lower(w.state) ELSE c.lifecycle END,
       c.author_node_id,c.author_login,NULL FROM canonical_objects c JOIN work_items w ON w.node_id=c.work_item_node_id;

-- Invalidation and an obligation to consider a change are orthogonal.
ALTER TABLE events ADD COLUMN requires_attention INTEGER NOT NULL DEFAULT 0
    CHECK (requires_attention IN (0,1));
UPDATE events SET requires_attention=1
WHERE kind IN ('wake','mention','assign');

CREATE INDEX events_object_version ON events(object_node_id,object_version)
    WHERE object_version IS NOT NULL;

-- A second transport observation can reuse a real existing event. Keep the
-- binding explicit instead of silently succeeding after INSERT OR IGNORE.
ALTER TABLE deliveries ADD COLUMN effect_event_id TEXT REFERENCES events(event_id);
UPDATE deliveries SET effect_event_id=(SELECT e.event_id FROM events e
    WHERE e.delivery_guid=deliveries.delivery_guid ORDER BY e.rowid LIMIT 1);

ALTER TABLE wake_batches ADD COLUMN input_error TEXT;

ALTER TABLE implementation_requests ADD COLUMN activation_event_id TEXT REFERENCES events(event_id);
UPDATE implementation_requests SET activation_event_id=(
    SELECT e.event_id FROM events e JOIN deliveries d ON d.delivery_guid=e.delivery_guid
    WHERE e.work_item_node_id=implementation_requests.pull_request_node_id
      AND e.kind='assign' AND e.detail='pr_ensure'
      AND d.delivery_guid='braid-pr-ensure-' || implementation_requests.repository || '-' || implementation_requests.comment_database_id
);

-- Exact version/content evidence of an Agent edit. The original author or a
-- human-visible attribution block cannot prove who made a later revision.
CREATE TABLE publication_versions (
    node_id TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL CHECK (length(digest)=64),
    intent_id TEXT NOT NULL REFERENCES write_intents(intent_id),
    PRIMARY KEY(node_id,version,digest)
) STRICT;
