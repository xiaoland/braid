ALTER TABLE provider_sessions
ADD COLUMN resume_count INTEGER NOT NULL DEFAULT 0 CHECK (resume_count >= 0);

ALTER TABLE provider_sessions ADD COLUMN last_resumed_at TEXT;
ALTER TABLE provider_sessions ADD COLUMN last_resume_error TEXT;

ALTER TABLE agent_instances
ADD COLUMN context_pressure TEXT NOT NULL DEFAULT 'normal'
CHECK (context_pressure IN ('normal', 'soft', 'hard', 'unavailable'));

ALTER TABLE agent_instances
ADD COLUMN context_bytes INTEGER CHECK (context_bytes IS NULL OR context_bytes >= 0);

ALTER TABLE agent_instances ADD COLUMN context_error TEXT;

DROP INDEX status_comments_remote_node;
ALTER TABLE status_comments RENAME TO status_comments_v6;

CREATE TABLE status_comments (
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    profile_id TEXT NOT NULL,
    assignment_generation INTEGER NOT NULL CHECK (assignment_generation > 0),
    remote_comment_node_id TEXT,
    remote_comment_database_id TEXT,
    write_intent_id TEXT UNIQUE,
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    lifecycle TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (work_item_node_id, profile_id, assignment_generation)
) STRICT;

INSERT INTO status_comments(
    work_item_node_id,profile_id,assignment_generation,remote_comment_node_id,
    remote_comment_database_id,write_intent_id,body_digest,lifecycle,updated_at
)
SELECT sc.work_item_node_id,sc.profile_id,
       COALESCE((
           SELECT MAX(a.generation)
           FROM assignments a
           JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
           WHERE a.work_item_node_id=sc.work_item_node_id
             AND ai.profile_id=sc.profile_id
       ),1),
       sc.remote_comment_node_id,sc.remote_comment_database_id,NULL,
       sc.body_digest,sc.lifecycle,sc.updated_at
FROM status_comments_v6 sc;

DROP TABLE status_comments_v6;

CREATE UNIQUE INDEX status_comments_remote_node
ON status_comments(remote_comment_node_id)
WHERE remote_comment_node_id IS NOT NULL;
