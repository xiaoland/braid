CREATE TABLE github_reaction_feedback (
    sequence INTEGER PRIMARY KEY,
    event_id TEXT UNIQUE REFERENCES events(event_id),
    turn_id TEXT REFERENCES turns(turn_id),
    repository TEXT NOT NULL,
    target_kind TEXT NOT NULL,
    target_database_id TEXT NOT NULL,
    content TEXT NOT NULL CHECK (content IN ('eyes','rocket','+1','confused','clear_rocket')),
    created_at TEXT NOT NULL,
    projected INTEGER NOT NULL DEFAULT 0 CHECK (projected IN (0,1)),
    publication_intent_id TEXT UNIQUE,
    UNIQUE (turn_id, content),
    CHECK ((event_id IS NOT NULL AND turn_id IS NULL AND content='eyes')
        OR (event_id IS NULL AND turn_id IS NOT NULL AND content!='eyes'))
) STRICT;

CREATE INDEX github_reaction_feedback_unprojected ON github_reaction_feedback(sequence)
WHERE projected=0;

CREATE INDEX github_reaction_feedback_clear ON github_reaction_feedback
(repository, target_kind, target_database_id, sequence)
WHERE content IN ('clear_rocket','+1','confused');

-- Existing writes keep their attempts and receipts. Copy their accepted event targets.
INSERT INTO github_reaction_feedback
    (event_id,repository,target_kind,target_database_id,content,created_at,projected,publication_intent_id)
SELECT event_id,repository,target_kind,target_database_id,content,created_at,1,intent_id
FROM github_write_outbox
WHERE event_id IS NOT NULL AND operation='reaction_add' AND content='eyes';
