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

CREATE UNIQUE INDEX assignments_active_work_item
ON assignments(work_item_node_id)
WHERE lifecycle IN ('materializing', 'active');

CREATE TABLE agent_instances (
    agent_id TEXT PRIMARY KEY,
    assignment_id TEXT NOT NULL REFERENCES assignments(assignment_id),
    profile_id TEXT NOT NULL,
    profile_revision INTEGER NOT NULL,
    role TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    context_pressure TEXT NOT NULL DEFAULT 'normal'
        CHECK (context_pressure IN ('normal', 'soft', 'hard', 'unavailable')),
    context_bytes INTEGER CHECK (context_bytes IS NULL OR context_bytes >= 0),
    context_error TEXT,
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
    resume_count INTEGER NOT NULL DEFAULT 0 CHECK (resume_count >= 0),
    last_resumed_at TEXT,
    last_resume_error TEXT,
    UNIQUE (provider_kind, provider_session_id)
) STRICT;

CREATE INDEX provider_sessions_lifecycle
ON provider_sessions(lifecycle, started_at);

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

CREATE TABLE turns (
    turn_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES provider_sessions(session_id),
    provider_turn_id TEXT,
    context_revision TEXT NOT NULL,
    trigger_kind TEXT NOT NULL,
    lifecycle TEXT NOT NULL,
    started_at TEXT,
    ended_at TEXT,
    batch_id TEXT REFERENCES wake_batches(batch_id)
) STRICT;

CREATE UNIQUE INDEX turns_batch
ON turns(batch_id)
WHERE batch_id IS NOT NULL;

CREATE INDEX turns_lifecycle
ON turns(lifecycle, started_at);

CREATE TABLE worktrees (
    worktree_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL UNIQUE REFERENCES agent_instances(agent_id),
    path TEXT NOT NULL UNIQUE,
    repository_node_id TEXT NOT NULL REFERENCES repositories(node_id),
    lifecycle TEXT NOT NULL,
    observed_at TEXT NOT NULL,
    source_path TEXT,
    head_ref TEXT,
    local_branch TEXT
) STRICT;

CREATE INDEX worktrees_repository_lifecycle
ON worktrees(repository_node_id, lifecycle);

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
    observed_at TEXT NOT NULL,
    database_id TEXT,
    author_login TEXT,
    reference_repository TEXT,
    reference_number INTEGER,
    pinned INTEGER NOT NULL DEFAULT 0 CHECK (pinned IN (0, 1))
) STRICT;

CREATE INDEX canonical_objects_work_item
ON canonical_objects(work_item_node_id, object_kind);

CREATE INDEX canonical_objects_lifecycle
ON canonical_objects(work_item_node_id, object_kind, lifecycle);

CREATE TABLE deliveries (
    delivery_guid TEXT PRIMARY KEY,
    repository_node_id TEXT REFERENCES repositories(node_id),
    event_name TEXT NOT NULL,
    action TEXT,
    received_at TEXT NOT NULL,
    admitted_at TEXT NOT NULL,
    repository_name TEXT,
    object_node_id TEXT,
    actor_node_id TEXT,
    actor_login TEXT,
    raw_payload BLOB NOT NULL DEFAULT X'',
    duplicate_count INTEGER NOT NULL DEFAULT 0 CHECK (duplicate_count >= 0),
    known INTEGER NOT NULL DEFAULT 1 CHECK (known IN (0, 1))
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
    observed_at TEXT NOT NULL,
    dedupe_key TEXT,
    mention_candidate INTEGER NOT NULL DEFAULT 0 CHECK (mention_candidate IN (0, 1)),
    trusted_mention INTEGER CHECK (trusted_mention IN (0, 1)),
    body_digest TEXT
) STRICT;

CREATE INDEX events_work_item_lifecycle
ON events(work_item_node_id, lifecycle);

CREATE UNIQUE INDEX events_dedupe_key
ON events(dedupe_key)
WHERE dedupe_key IS NOT NULL;

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

CREATE UNIQUE INDEX github_write_outbox_request
ON github_write_outbox(request_digest);

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
    updated_at TEXT NOT NULL,
    request_key TEXT,
    repository TEXT,
    target TEXT,
    profile_id TEXT,
    role TEXT,
    payload TEXT,
    remote_database_id TEXT,
    remote_url TEXT,
    last_error TEXT,
    claim_expires_at TEXT
) STRICT;

CREATE INDEX write_intents_lifecycle
ON write_intents(lifecycle, updated_at);

CREATE UNIQUE INDEX write_intents_request_key
ON write_intents(request_key)
WHERE request_key IS NOT NULL;

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
    assignment_generation INTEGER NOT NULL CHECK (assignment_generation > 0),
    remote_comment_node_id TEXT,
    remote_comment_database_id TEXT,
    write_intent_id TEXT UNIQUE,
    body_digest TEXT NOT NULL CHECK (length(body_digest) = 64),
    lifecycle TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (work_item_node_id, profile_id, assignment_generation)
) STRICT;

CREATE UNIQUE INDEX status_comments_remote_node
ON status_comments(remote_comment_node_id)
WHERE remote_comment_node_id IS NOT NULL;

CREATE TABLE sync_cursors (
    repository_node_id TEXT NOT NULL REFERENCES repositories(node_id),
    surface TEXT NOT NULL,
    cursor TEXT,
    observed_at TEXT NOT NULL,
    PRIMARY KEY (repository_node_id, surface)
) STRICT;

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

CREATE TABLE issue_context_sources (
    issue_node_id TEXT PRIMARY KEY REFERENCES work_items(node_id),
    visible_description TEXT NOT NULL,
    observed_at TEXT NOT NULL
) STRICT;

CREATE TABLE context_resets (
    reset_id TEXT PRIMARY KEY,
    agent_id TEXT NOT NULL REFERENCES agent_instances(agent_id),
    old_session_id TEXT NOT NULL REFERENCES provider_sessions(session_id),
    active_turn_id TEXT REFERENCES turns(turn_id),
    new_session_id TEXT REFERENCES provider_sessions(session_id),
    context_revision_before TEXT NOT NULL,
    context_revision_after TEXT,
    continuation INTEGER NOT NULL CHECK (continuation IN (0, 1)),
    lifecycle TEXT NOT NULL CHECK (
        lifecycle IN ('interrupting', 'materializing', 'applied', 'blocked')
    ),
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE UNIQUE INDEX context_resets_active_agent
ON context_resets(agent_id)
WHERE lifecycle IN ('interrupting', 'materializing');

CREATE TABLE context_reset_events (
    reset_id TEXT NOT NULL REFERENCES context_resets(reset_id),
    event_id TEXT PRIMARY KEY REFERENCES events(event_id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0)
) STRICT;
