CREATE TABLE operational_comment_roles (
    role_id TEXT PRIMARY KEY CHECK (length(role_id) = 32),
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    profile_id TEXT NOT NULL,
    assignment_generation INTEGER NOT NULL CHECK (assignment_generation > 0),
    comment_node_id TEXT UNIQUE,
    ambiguous INTEGER NOT NULL DEFAULT 0 CHECK (ambiguous IN (0, 1)),
    UNIQUE (work_item_node_id, profile_id, assignment_generation)
) STRICT;

CREATE TABLE operational_comment_role_bodies (
    role_id TEXT NOT NULL REFERENCES operational_comment_roles(role_id),
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    PRIMARY KEY (role_id, body_digest)
) STRICT;

CREATE TABLE legacy_operational_comment_bodies (
    role_id TEXT NOT NULL REFERENCES operational_comment_roles(role_id),
    body TEXT NOT NULL,
    not_before TEXT NOT NULL,
    PRIMARY KEY (role_id, body, not_before)
) STRICT;

-- Preserve exact historical identities and recorded multiple-match uncertainty.
INSERT INTO operational_comment_roles
    (role_id, work_item_node_id, profile_id, assignment_generation, comment_node_id, ambiguous)
SELECT lower(hex(randomblob(16))), s.work_item_node_id, s.profile_id, s.assignment_generation,
       s.remote_comment_node_id, s.remote_comment_node_id IS NULL AND EXISTS (
           SELECT 1 FROM github_write_outbox o
           WHERE o.status_work_item_node_id=s.work_item_node_id
             AND o.status_profile_id=s.profile_id AND o.status_generation=s.assignment_generation
             AND o.operation='comment_create' AND o.lifecycle='ambiguous')
FROM status_comments s;

-- A pending request with zero attempts has never crossed the network boundary.
UPDATE github_write_outbox AS o
SET content=o.content || char(10,10) || '<!-- braid-operational-status:v1:' || (
    SELECT r.role_id FROM operational_comment_roles r
    WHERE r.work_item_node_id=o.status_work_item_node_id
      AND r.profile_id=o.status_profile_id AND r.assignment_generation=o.status_generation
) || ' -->'
WHERE o.status_work_item_node_id IS NOT NULL
  AND o.operation IN ('comment_create','comment_update')
  AND o.lifecycle='pending' AND o.attempts=0;

-- Save the recovery evidence independently of future publisher activity.
INSERT OR IGNORE INTO legacy_operational_comment_bodies (role_id, body, not_before)
SELECT r.role_id, o.content, o.created_at
FROM github_write_outbox o JOIN operational_comment_roles r
  ON r.work_item_node_id=o.status_work_item_node_id
 AND r.profile_id=o.status_profile_id AND r.assignment_generation=o.status_generation
WHERE r.comment_node_id IS NULL AND r.ambiguous=0
  AND o.operation='comment_create' AND o.lifecycle IN ('pending','sending','uncertain','applied');
