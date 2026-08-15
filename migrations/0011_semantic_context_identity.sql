CREATE TABLE associations_v11 (
    issue_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    pr_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    source TEXT NOT NULL,
    observed_version TEXT NOT NULL,
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    PRIMARY KEY (issue_node_id, pr_node_id)
) STRICT;

INSERT INTO associations_v11(
    issue_node_id,pr_node_id,source,observed_version,active
)
SELECT issue_node_id,pr_node_id,source,observed_version,active
FROM associations;

DROP TABLE associations;
ALTER TABLE associations_v11 RENAME TO associations;

CREATE TABLE issue_context_sources (
    issue_node_id TEXT PRIMARY KEY REFERENCES work_items(node_id),
    visible_description TEXT NOT NULL,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE canonical_objects_v11 (
    node_id TEXT PRIMARY KEY,
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    object_kind TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    lifecycle TEXT NOT NULL,
    author_node_id TEXT,
    created_at TEXT,
    updated_at TEXT,
    observed_at TEXT NOT NULL,
    database_id TEXT,
    author_login TEXT,
    reference_repository TEXT,
    reference_number INTEGER,
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1))
) STRICT;

INSERT INTO canonical_objects_v11(
    node_id,work_item_node_id,object_kind,version,digest,lifecycle,
    author_node_id,created_at,updated_at,observed_at,database_id,
    author_login,reference_repository,reference_number,pinned
)
SELECT node_id,work_item_node_id,object_kind,version,digest,lifecycle,
       author_node_id,created_at,updated_at,observed_at,database_id,
       author_login,reference_repository,reference_number,pinned
FROM canonical_objects;

DROP TABLE canonical_objects;
ALTER TABLE canonical_objects_v11 RENAME TO canonical_objects;

CREATE INDEX canonical_objects_lifecycle
ON canonical_objects(work_item_node_id, object_kind, lifecycle);
