CREATE TABLE harness_runs (
    run_id TEXT PRIMARY KEY NOT NULL,
    request_json TEXT NOT NULL,
    result_json TEXT NOT NULL
);
