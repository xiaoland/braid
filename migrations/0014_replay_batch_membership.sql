-- An uncertain turn retains its original input evidence while a later batch
-- re-delivers those same events after the old execution is confirmed stopped.
CREATE TABLE wake_batch_events_v14 (
    batch_id TEXT NOT NULL REFERENCES wake_batches(batch_id),
    event_id TEXT NOT NULL REFERENCES events(event_id),
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    PRIMARY KEY (batch_id, ordinal),
    UNIQUE (batch_id, event_id)
) STRICT;
INSERT INTO wake_batch_events_v14(batch_id,event_id,ordinal)
SELECT batch_id,event_id,ordinal FROM wake_batch_events;
DROP TABLE wake_batch_events;
ALTER TABLE wake_batch_events_v14 RENAME TO wake_batch_events;
CREATE INDEX wake_batch_events_event ON wake_batch_events(event_id);
