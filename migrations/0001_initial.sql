CREATE TABLE schema_migrations (
    version INTEGER PRIMARY KEY CHECK (version > 0),
    name TEXT NOT NULL UNIQUE,
    checksum TEXT NOT NULL CHECK (length(checksum) = 64),
    applied_at TEXT NOT NULL
) STRICT;

CREATE TABLE owner_leases (
    scope TEXT PRIMARY KEY,
    generation INTEGER NOT NULL CHECK (generation >= 0),
    owner_id TEXT NOT NULL,
    expires_at TEXT NOT NULL
) STRICT;

CREATE TABLE repositories (
    node_id TEXT PRIMARY KEY,
    name_with_owner TEXT NOT NULL UNIQUE,
    installation_id INTEGER,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE work_items (
    node_id TEXT PRIMARY KEY,
    repository_node_id TEXT NOT NULL REFERENCES repositories(node_id),
    kind TEXT NOT NULL CHECK (kind IN ('issue', 'pr')),
    number INTEGER NOT NULL CHECK (number > 0),
    state TEXT NOT NULL,
    context_revision TEXT,
    observed_at TEXT NOT NULL,
    UNIQUE (repository_node_id, kind, number)
) STRICT;

CREATE TABLE profiles (
    profile_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    effective_digest TEXT NOT NULL CHECK (length(effective_digest) = 64),
    provider_kind TEXT NOT NULL,
    tags TEXT NOT NULL,
    PRIMARY KEY (profile_id, revision)
) STRICT;

CREATE TABLE assignments (
    assignment_id TEXT PRIMARY KEY,
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    generation INTEGER NOT NULL CHECK (generation > 0),
    lifecycle TEXT NOT NULL,
    assigned_at TEXT NOT NULL,
    retired_at TEXT,
    UNIQUE (work_item_node_id, generation)
) STRICT;

CREATE TABLE agent_instances (
    agent_id TEXT PRIMARY KEY,
    assignment_id TEXT NOT NULL REFERENCES assignments(assignment_id),
    profile_id TEXT NOT NULL,
    profile_revision INTEGER NOT NULL,
    role TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    FOREIGN KEY (profile_id, profile_revision) REFERENCES profiles(profile_id, revision)
) STRICT;

CREATE TABLE provider_sessions (
    session_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agent_instances(agent_id),
    provider_kind TEXT NOT NULL,
    provider_session_id TEXT NOT NULL,
    context_revision TEXT NOT NULL,
    instruction_revision TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    started_at TEXT NOT NULL,
    UNIQUE (provider_kind, provider_session_id)
) STRICT;

CREATE TABLE turns (
    turn_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES provider_sessions(session_id),
    provider_turn_id TEXT,
    context_revision TEXT NOT NULL,
    trigger_kind TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT
) STRICT;

CREATE TABLE worktrees (
    worktree_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL UNIQUE REFERENCES agent_instances(agent_id),
    path TEXT NOT NULL UNIQUE,
    repository_node_id TEXT NOT NULL REFERENCES repositories(node_id),
    lifecycle TEXT NOT NULL,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE associations (
    issue_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    pr_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    source TEXT NOT NULL,
    observed_version TEXT NOT NULL,
    active INTEGER NOT NULL CHECK (active IN (0, 1)),
    PRIMARY KEY (issue_node_id, pr_node_id)
) STRICT;

CREATE TABLE canonical_objects (
    node_id TEXT PRIMARY KEY,
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    object_kind TEXT NOT NULL,
    version TEXT NOT NULL,
    digest TEXT NOT NULL CHECK (length(digest) = 64),
    lifecycle TEXT NOT NULL,
    author_node_id TEXT,
    created_at TEXT,
    updated_at TEXT,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE deliveries (
    delivery_guid TEXT PRIMARY KEY,
    repository_node_id TEXT REFERENCES repositories(node_id),
    event_name TEXT NOT NULL,
    action TEXT,
    received_at TEXT NOT NULL,
    admitted_at TEXT NOT NULL
) STRICT;

CREATE TABLE events (
    event_id TEXT PRIMARY KEY,
    delivery_guid TEXT REFERENCES deliveries(delivery_guid),
    work_item_node_id TEXT REFERENCES work_items(node_id),
    object_node_id TEXT,
    object_version TEXT,
    classification TEXT NOT NULL,
    origin TEXT NOT NULL,
    reference TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE scheduler_batches (
    batch_id TEXT PRIMARY KEY,
    assignment_id TEXT NOT NULL REFERENCES assignments(assignment_id),
    profile_id TEXT NOT NULL,
    profile_revision INTEGER NOT NULL,
    context_revision TEXT,
    event_count INTEGER NOT NULL DEFAULT 0 CHECK (event_count >= 0),
    quiet_deadline TEXT,
    urgent INTEGER NOT NULL DEFAULT 0 CHECK (urgent IN (0, 1)),
    lifecycle TEXT NOT NULL,
    FOREIGN KEY (profile_id, profile_revision) REFERENCES profiles(profile_id, revision)
) STRICT;

CREATE TABLE batch_events (
    batch_id TEXT NOT NULL REFERENCES scheduler_batches(batch_id),
    event_id TEXT NOT NULL UNIQUE REFERENCES events(event_id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (batch_id, ordinal)
) STRICT;

CREATE TABLE write_intents (
    intent_id TEXT PRIMARY KEY,
    agent_id TEXT REFERENCES agent_instances(agent_id),
    work_item_node_id TEXT REFERENCES work_items(node_id),
    operation TEXT NOT NULL,
    request_digest TEXT NOT NULL CHECK (length(request_digest) = 64),
    remote_node_id TEXT,
    lifecycle TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE reaction_targets (
    target_node_id TEXT NOT NULL,
    reaction TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    desired INTEGER NOT NULL CHECK (desired IN (0, 1)),
    lifecycle TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (target_node_id, reaction, generation)
) STRICT;

CREATE TABLE status_comments (
    work_item_node_id TEXT NOT NULL REFERENCES work_items(node_id),
    profile_id TEXT NOT NULL,
    remote_comment_node_id TEXT,
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    lifecycle TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (work_item_node_id, profile_id)
) STRICT;

CREATE TABLE sync_cursors (
    repository_node_id TEXT NOT NULL REFERENCES repositories(node_id),
    surface TEXT NOT NULL,
    cursor TEXT,
    observed_at TEXT NOT NULL,
    PRIMARY KEY (repository_node_id, surface)
) STRICT;

CREATE INDEX events_work_item_lifecycle ON events(work_item_node_id, lifecycle);
CREATE INDEX canonical_objects_work_item ON canonical_objects(work_item_node_id, object_kind);
CREATE INDEX write_intents_lifecycle ON write_intents(lifecycle, updated_at);
