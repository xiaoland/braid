-- Preserve complete operational failure evidence before publishing a bounded summary.
ALTER TABLE turns ADD COLUMN error TEXT;
