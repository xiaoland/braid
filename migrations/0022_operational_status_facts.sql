CREATE TABLE operational_status_facts (
    assignment_id TEXT PRIMARY KEY REFERENCES assignments(assignment_id),
    body TEXT NOT NULL,
    observed_at TEXT NOT NULL
) STRICT;
