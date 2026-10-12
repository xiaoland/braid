CREATE TABLE builtin_upstream (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 space_id TEXT NOT NULL,
 repository TEXT NOT NULL,
 client_id TEXT NOT NULL,
 cursor INTEGER NOT NULL DEFAULT 0 CHECK(cursor>=0)
);
CREATE TABLE builtin_writer_grants (
 session_id TEXT PRIMARY KEY,
 item_number INTEGER NOT NULL,
 actor TEXT NOT NULL,
 request_id TEXT NOT NULL,
 epoch INTEGER NOT NULL,
 grant_id TEXT,
 token TEXT,
 lifecycle TEXT NOT NULL CHECK(lifecycle IN ('pending','active','revoked'))
);
