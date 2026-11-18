-- A later comment may refer to a published managed implementation without
-- creating another PR. The original immutable request remains its identity.
ALTER TABLE implementation_requests ADD COLUMN root_intent_id TEXT
    REFERENCES implementation_requests(intent_id);
CREATE INDEX implementation_requests_managed_head
ON implementation_requests(repository,head_ref,issue_node_id,base_ref,pr_profile_id);

-- Legacy false success is an unfinished, already-authorized delivery obligation.
-- Reuse the one outbox; execution revalidates the exact published PR/association
-- and refuses to replace any blocked/sleeping/retired assignment generation.
UPDATE write_intents SET lifecycle='pending',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'),
    last_error='published implementation still owes activation delivery'
WHERE operation='pr_ensure' AND lifecycle='applied' AND intent_id IN (
    SELECT intent_id FROM implementation_requests
    WHERE activation_event_id IS NULL AND stage='activation_pending'
      AND pull_request_node_id IS NOT NULL AND pull_request_number IS NOT NULL
);
INSERT OR IGNORE INTO github_write_outbox(intent_id,repository,target_kind,target_database_id,
    operation,content,request_digest,lifecycle,next_attempt_at,created_at,updated_at)
SELECT intent_id,repository,'agent',target,'agent_write',payload,lower(hex(randomblob(32))),
    'pending',updated_at,created_at,updated_at FROM write_intents
WHERE operation='pr_ensure' AND lifecycle='pending' AND intent_id IN (
    SELECT intent_id FROM implementation_requests WHERE activation_event_id IS NULL
      AND stage='activation_pending' AND pull_request_node_id IS NOT NULL
);
UPDATE github_write_outbox SET lifecycle='pending',
    next_attempt_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'),
    updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now'),
    last_error='published implementation still owes activation delivery'
WHERE lifecycle='applied' AND intent_id IN (
    SELECT w.intent_id FROM write_intents w JOIN implementation_requests i ON i.intent_id=w.intent_id
    WHERE w.operation='pr_ensure' AND w.lifecycle='pending' AND i.activation_event_id IS NULL
      AND i.stage='activation_pending' AND i.pull_request_node_id IS NOT NULL AND i.pull_request_number IS NOT NULL
);
