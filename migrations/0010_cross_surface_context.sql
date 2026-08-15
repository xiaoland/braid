ALTER TABLE canonical_objects ADD COLUMN content_digest TEXT;
ALTER TABLE associations ADD COLUMN issue_content_digest TEXT;
