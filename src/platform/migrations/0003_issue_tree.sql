CREATE TABLE local_issue_parents (
    child_number INTEGER PRIMARY KEY REFERENCES local_items(number),
    parent_number INTEGER NOT NULL REFERENCES local_items(number),
    CHECK(child_number != parent_number)
) STRICT;
CREATE INDEX local_issue_parents_parent ON local_issue_parents(parent_number,child_number);
CREATE TABLE run_root_policies (
    root_number INTEGER PRIMARY KEY REFERENCES local_items(number),
    delivery_ref TEXT NOT NULL,
    run_id TEXT NOT NULL,
    client_id TEXT NOT NULL
) STRICT;
