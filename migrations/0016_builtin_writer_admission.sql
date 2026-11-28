-- A native turn can run before a writer grant is requested.
-- Legacy turns have no recorded authority snapshot and cannot mint a new grant.
ALTER TABLE turns ADD COLUMN builtin_writer_cursor INTEGER CHECK (builtin_writer_cursor >= 0);
ALTER TABLE builtin_writer_grants ADD COLUMN authority_cursor INTEGER CHECK (authority_cursor >= 0);
ALTER TABLE builtin_writer_grants ADD COLUMN revoking INTEGER NOT NULL DEFAULT 0 CHECK (revoking IN (0, 1));
