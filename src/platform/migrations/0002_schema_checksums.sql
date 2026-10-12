-- Version 1 predates checksums. Preserve its ledger row and add verification metadata.
ALTER TABLE platform_schema_migrations ADD COLUMN name TEXT;
ALTER TABLE platform_schema_migrations ADD COLUMN checksum TEXT CHECK(checksum IS NULL OR length(checksum)=64);
ALTER TABLE platform_schema_migrations ADD COLUMN applied_at TEXT;
CREATE UNIQUE INDEX platform_schema_migration_names ON platform_schema_migrations(name);
