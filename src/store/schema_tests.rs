use super::{DATABASE_SCHEMA_VERSION, StoreActor, StoreError};
use rusqlite::Connection;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug)]
enum InvalidSchema {
    Foreign,
    Newer,
    NonContiguous,
    ChangedChecksum,
}

struct TemporaryRoot(PathBuf);
impl Drop for TemporaryRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn migration_entry_points_reject_incompatible_databases_without_changing_bytes() {
    for case in [
        InvalidSchema::Foreign,
        InvalidSchema::Newer,
        InvalidSchema::NonContiguous,
        InvalidSchema::ChangedChecksum,
    ] {
        let root = TemporaryRoot(
            std::env::temp_dir().join(format!("braid-schema-{}", uuid::Uuid::now_v7())),
        );
        std::fs::create_dir_all(&root.0).unwrap();
        let database = root.0.join("state.sqlite3");
        let backups = root.0.join("backups");
        if !matches!(case, InvalidSchema::Foreign) {
            let store = StoreActor::start(database.clone(), backups.clone()).unwrap();
            assert_eq!(store.apply().unwrap().current_schema, DATABASE_SCHEMA_VERSION);
            assert!(store.plan().unwrap().pending.is_empty());
            drop(store);
        }
        let db = Connection::open(&database).unwrap();
        match case {
            InvalidSchema::Foreign => {
                db.execute_batch(
                    "CREATE TABLE user_data(value TEXT NOT NULL);
                     INSERT INTO user_data VALUES ('preserve this foreign database');",
                )
                .unwrap();
            }
            InvalidSchema::Newer => {
                db.execute(
                    "INSERT INTO schema_migrations(version,name,checksum,applied_at)
                     VALUES (?1,'future-schema',?2,'2026-01-01T00:00:00Z')",
                    rusqlite::params![DATABASE_SCHEMA_VERSION + 1, "0".repeat(64)],
                )
                .unwrap();
            }
            InvalidSchema::NonContiguous => {
                assert_eq!(
                    db.execute("DELETE FROM schema_migrations WHERE version=2", []).unwrap(),
                    1
                );
            }
            InvalidSchema::ChangedChecksum => {
                assert_eq!(
                    db.execute(
                        "UPDATE schema_migrations SET checksum=?1 WHERE version=1 AND checksum<>?1",
                        ["0".repeat(64)],
                    )
                    .unwrap(),
                    1
                );
            }
        }
        // Include every committed page in the snapshot, with no WAL left outside it.
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").unwrap();
        drop(db);
        let before = std::fs::read(&database).unwrap();
        assert!(!before.is_empty());
        let store = StoreActor::start(database.clone(), backups).unwrap();
        for apply in [false, true] {
            let result = if apply { store.apply().map(|_| ()) } else { store.plan().map(|_| ()) };
            let error = result.expect_err("invalid schema must be rejected before migration");
            match (case, error) {
                (InvalidSchema::Foreign, StoreError::ForeignDatabase { path }) => {
                    assert_eq!(path, database);
                }
                (InvalidSchema::Newer, StoreError::NewerSchema { found, supported }) => {
                    assert_eq!(found, DATABASE_SCHEMA_VERSION + 1);
                    assert_eq!(supported, DATABASE_SCHEMA_VERSION);
                }
                (InvalidSchema::NonContiguous, StoreError::NonContiguous { version }) => {
                    assert_eq!(version, 3);
                }
                (InvalidSchema::ChangedChecksum, StoreError::ChecksumMismatch { version }) => {
                    assert_eq!(version, 1);
                }
                (case, error) => panic!("wrong rejection for {case:?}, apply={apply}: {error:?}"),
            }
            assert_eq!(std::fs::read(&database).unwrap(), before, "{case:?}, apply={apply}");
        }
        drop(store);
    }
}
