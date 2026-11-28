-- A complete canonical observation owns the title/body comparison frontier.
-- Legacy rows carry only the combined projection. The next complete snapshot
-- establishes the field baseline without inferring authorship from receipts.
ALTER TABLE processed_objects ADD COLUMN title_digest TEXT CHECK (length(title_digest)=64);
ALTER TABLE processed_objects ADD COLUMN body_digest TEXT CHECK (length(body_digest)=64);
ALTER TABLE processed_objects ADD COLUMN title_event_id TEXT;
