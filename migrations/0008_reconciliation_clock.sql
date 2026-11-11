-- Full traversal success is independent of ordinary webhook/context observations.
ALTER TABLE work_items ADD COLUMN reconciled_at INTEGER;
ALTER TABLE work_items ADD COLUMN reconcile_attempted_at INTEGER;
