ALTER TABLE write_intents ADD COLUMN request_key TEXT;
ALTER TABLE write_intents ADD COLUMN repository TEXT;
ALTER TABLE write_intents ADD COLUMN target TEXT;
ALTER TABLE write_intents ADD COLUMN profile_id TEXT;
ALTER TABLE write_intents ADD COLUMN role TEXT;
ALTER TABLE write_intents ADD COLUMN payload TEXT;
ALTER TABLE write_intents ADD COLUMN remote_database_id TEXT;
ALTER TABLE write_intents ADD COLUMN remote_url TEXT;
ALTER TABLE write_intents ADD COLUMN last_error TEXT;
ALTER TABLE write_intents ADD COLUMN claim_expires_at TEXT;

CREATE UNIQUE INDEX write_intents_request_key
ON write_intents(request_key)
WHERE request_key IS NOT NULL;

CREATE TABLE implementation_requests (
    intent_id TEXT PRIMARY KEY REFERENCES write_intents(intent_id),
    repository TEXT NOT NULL,
    comment_database_id INTEGER NOT NULL CHECK (comment_database_id > 0),
    comment_node_id TEXT NOT NULL,
    issue_node_id TEXT NOT NULL,
    issue_number INTEGER NOT NULL CHECK (issue_number > 0),
    issue_title TEXT NOT NULL,
    base_ref TEXT NOT NULL,
    head_ref TEXT NOT NULL,
    pr_profile_id TEXT NOT NULL,
    bootstrap_authored_at TEXT NOT NULL,
    bootstrap_commit_sha TEXT,
    pull_request_database_id INTEGER CHECK (
        pull_request_database_id IS NULL OR pull_request_database_id > 0
    ),
    pull_request_node_id TEXT,
    pull_request_number INTEGER CHECK (
        pull_request_number IS NULL OR pull_request_number > 0
    ),
    stage TEXT NOT NULL CHECK (
        stage IN ('planned', 'head_ready', 'pull_request_ready', 'associated', 'activation_pending')
    ),
    updated_at TEXT NOT NULL,
    UNIQUE (repository, comment_database_id)
) STRICT;
