CREATE TABLE control_receipts (
    scope TEXT NOT NULL,
    request_id TEXT NOT NULL,
    payload_sha256 TEXT NOT NULL CHECK(length(payload_sha256)=64),
    request_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('prepared','applied','rejected','blocked')),
    response_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY(scope,request_id)
) STRICT;
