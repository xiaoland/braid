CREATE TABLE local_items (
    number INTEGER PRIMARY KEY AUTOINCREMENT,
    repository TEXT NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('issue','pr')),
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    author TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'open' CHECK(state IN ('open','closed')),
    desired_profile TEXT,
    head_ref TEXT,
    base_ref TEXT,
    head_commit TEXT,
    base_commit TEXT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK((kind='issue' AND head_ref IS NULL AND base_ref IS NULL AND head_commit IS NULL AND base_commit IS NULL)
       OR (kind='pr' AND head_ref IS NOT NULL AND base_ref IS NOT NULL AND head_commit IS NOT NULL AND base_commit IS NOT NULL))
) STRICT;
CREATE INDEX local_items_repository ON local_items(repository,number);
CREATE TABLE local_comments (
    comment_id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_number INTEGER NOT NULL REFERENCES local_items(number),
    author TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;
CREATE TABLE local_links (
    issue_number INTEGER NOT NULL REFERENCES local_items(number),
    pr_number INTEGER NOT NULL REFERENCES local_items(number),
    PRIMARY KEY(issue_number,pr_number)
) STRICT;
-- Replay uses the stable event ID as the existing ingress delivery GUID.
CREATE TABLE local_events (
    event_id INTEGER PRIMARY KEY AUTOINCREMENT,
    item_number INTEGER NOT NULL REFERENCES local_items(number),
    kind TEXT NOT NULL,
    detail TEXT,
    actor TEXT NOT NULL,
    body TEXT,
    revision INTEGER NOT NULL,
    item_state TEXT NOT NULL,
    created_at TEXT NOT NULL,
    consumed_at TEXT
) STRICT;
CREATE INDEX local_events_pending ON local_events(event_id) WHERE consumed_at IS NULL;
CREATE TABLE local_space (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    repository TEXT NOT NULL,
    source TEXT
) STRICT;
ALTER TABLE local_comments ADD COLUMN operational INTEGER NOT NULL DEFAULT 0 CHECK(operational IN (0,1));
ALTER TABLE local_events ADD COLUMN origin TEXT NOT NULL DEFAULT 'human';
CREATE TABLE local_reactions (
    reaction_id INTEGER PRIMARY KEY AUTOINCREMENT,
    comment_id INTEGER NOT NULL REFERENCES local_comments(comment_id),
    content TEXT NOT NULL,
    UNIQUE(comment_id,content)
) STRICT;
CREATE TABLE local_write_receipts (
    intent_id TEXT PRIMARY KEY,
    database_id TEXT,
    node_id TEXT
) STRICT;
CREATE TABLE local_merges (
    pr_number INTEGER PRIMARY KEY REFERENCES local_items(number),
    expected_base TEXT NOT NULL,
    expected_head TEXT NOT NULL,
    merge_commit TEXT,
    lifecycle TEXT NOT NULL CHECK(lifecycle IN ('prepared','applied','conflict','failed')),
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
ALTER TABLE local_items ADD COLUMN published INTEGER NOT NULL DEFAULT 0 CHECK(published IN (0,1));
ALTER TABLE local_comments ADD COLUMN reply_to INTEGER REFERENCES local_comments(comment_id);
