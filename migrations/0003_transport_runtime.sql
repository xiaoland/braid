ALTER TABLE deliveries ADD COLUMN repository_name TEXT;
ALTER TABLE deliveries ADD COLUMN object_node_id TEXT;
ALTER TABLE deliveries ADD COLUMN actor_node_id TEXT;
ALTER TABLE deliveries ADD COLUMN actor_login TEXT;
ALTER TABLE deliveries ADD COLUMN raw_payload BLOB NOT NULL DEFAULT X'';
ALTER TABLE deliveries ADD COLUMN duplicate_count INTEGER NOT NULL DEFAULT 0 CHECK (duplicate_count >= 0);
ALTER TABLE deliveries ADD COLUMN known INTEGER NOT NULL DEFAULT 1 CHECK (known IN (0, 1));

ALTER TABLE events ADD COLUMN dedupe_key TEXT;
ALTER TABLE events ADD COLUMN mention_candidate INTEGER NOT NULL DEFAULT 0 CHECK (mention_candidate IN (0, 1));
ALTER TABLE events ADD COLUMN trusted_mention INTEGER CHECK (trusted_mention IN (0, 1));
ALTER TABLE events ADD COLUMN body_digest TEXT;

CREATE UNIQUE INDEX events_dedupe_key
ON events(dedupe_key)
WHERE dedupe_key IS NOT NULL;

CREATE TABLE wake_batches (
    batch_id TEXT PRIMARY KEY,
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    context_revision TEXT,
    event_count INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
    quiet_deadline TEXT NOT NULL,
    urgent INTEGER NOT NULL DEFAULT 0 CHECK (urgent IN (0, 1)),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('pending', 'runnable', 'consumed')),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX wake_batches_open_work_item
ON wake_batches(work_item_node_id)
WHERE lifecycle IN ('pending', 'runnable');

CREATE TABLE wake_batch_events (
    batch_id TEXT NOT NULL REFERENCES wake_batches(batch_id),
    event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (batch_id, ordinal)
) STRICT;

CREATE TABLE github_write_outbox (
    intent_id TEXT PRIMARY KEY,
    event_id TEXT UNIQUE REFERENCES events(event_id),
    repository TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_database_id TEXT NOT NULL,
    operation TEXT NOT NULL,
    content TEXT NOT NULL,
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
    remote_database_id TEXT,
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN ('pending', 'sending', 'uncertain', 'applied', 'conflict', 'ambiguous', 'rejected', 'superseded')
    ),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    next_attempt_at TEXT NOT NULL,
    last_error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE INDEX github_write_outbox_lifecycle
ON github_write_outbox(lifecycle, next_attempt_at);

CREATE TABLE reconciliation_runs (
    run_id TEXT PRIMARY KEY,
    repository_node_id TEXT NOT NULL REFERENCES repositories(node_id),
    lifecycle TEXT NOT NULL CHECK (lifecycle IN ('running', 'completed', 'failed')),
    started_at TEXT NOT NULL,
    completed_at TEXT,
    work_item_count INTEGER NOT NULL DEFAULT 0 CHECK (work_item_count >= 0),
    change_count INTEGER NOT NULL DEFAULT 0 CHECK (change_count >= 0),
    error TEXT
) STRICT;

CREATE INDEX reconciliation_runs_repository_started
ON reconciliation_runs(repository_node_id, started_at DESC);
