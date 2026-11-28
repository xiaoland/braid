//! Full traversal clocks: ordinary observations never advance these facts.
use rusqlite::{Connection, params};

#[derive(Debug, Clone)]
pub struct TrackedWorkItem {
    pub node_id: String,
    pub repository: String,
    pub kind: String,
    pub number: u64,
    pub state: String,
}

/// # Errors
/// Returns `SQLite` read or invalid Work Item number errors.
pub fn due(
    connection: &Connection,
    now: i64,
    hot_seconds: u64,
) -> rusqlite::Result<Vec<TrackedWorkItem>> {
    let mut statement = connection.prepare(
        "SELECT w.node_id,r.name_with_owner,w.kind,w.number,w.state
         FROM work_items w JOIN repositories r ON r.node_id=w.repository_node_id
         WHERE (w.reconcile_attempted_at IS NULL OR w.reconcile_attempted_at <= ?1 - ?2)
           AND (w.reconciled_at IS NULL OR w.reconciled_at <= ?1 - CASE
             WHEN upper(w.state)='OPEN' OR EXISTS (
               SELECT 1 FROM assignments a WHERE a.work_item_node_id=w.node_id
                 AND a.lifecycle IN ('materializing','active','finalizing')) THEN ?2 ELSE 1800 END)
         ORDER BY coalesce(w.reconcile_attempted_at,0),coalesce(w.reconciled_at,0),w.node_id",
    )?;
    statement
        .query_map(params![now, i64::try_from(hot_seconds).unwrap_or(i64::MAX)], |row| {
            Ok(TrackedWorkItem {
                node_id: row.get(0)?,
                repository: row.get(1)?,
                kind: row.get(2)?,
                number: u64::try_from(row.get::<_, i64>(3)?).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Integer,
                        Box::new(error),
                    )
                })?,
                state: row.get(4)?,
            })
        })?
        .collect()
}

/// # Errors
/// Returns `SQLite` update errors; failed/partial attempts do not advance success.
pub fn record(
    connection: &Connection,
    node_id: &str,
    now: i64,
    complete: bool,
) -> rusqlite::Result<()> {
    connection.execute(
        "UPDATE work_items SET reconcile_attempted_at=?2,
         reconciled_at=CASE WHEN ?3 THEN ?2 ELSE reconciled_at END WHERE node_id=?1",
        params![node_id, now, complete],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database(db: Connection) -> Connection {
        db.execute_batch("CREATE TABLE repositories(node_id TEXT,name_with_owner TEXT);
          CREATE TABLE work_items(node_id TEXT,repository_node_id TEXT,kind TEXT,number INTEGER,state TEXT,observed_at INTEGER);
          CREATE TABLE assignments(work_item_node_id TEXT,lifecycle TEXT);
          INSERT INTO repositories VALUES('r','o/r');
          INSERT INTO work_items VALUES('open','r','issue',1,'OPEN',0),('closed','r','pr',2,'CLOSED',0),('final','r','pr',3,'MERGED',0);
          INSERT INTO assignments VALUES('final','finalizing');").unwrap();
        db.execute_batch(include_str!("../../migrations/0008_reconciliation_clock.sql")).unwrap();
        db
    }

    fn due_ids(db: &Connection, now: i64) -> Vec<String> {
        let mut items: Vec<_> =
            due(db, now, 60).unwrap().into_iter().map(|item| item.node_id).collect();
        items.sort_unstable();
        items
    }

    #[test]
    fn traversal_deadlines_survive_observations_and_restart() {
        let path = std::env::temp_dir()
            .join(format!("braid-reconciliation-{}.sqlite", uuid::Uuid::now_v7()));
        let db = database(Connection::open(&path).unwrap());
        for id in ["open", "closed", "final"] {
            record(&db, id, 100, true).unwrap();
        }
        db.execute("UPDATE work_items SET observed_at=500", []).unwrap();
        drop(db);
        let db = Connection::open(&path).unwrap();
        assert!(due_ids(&db, 159).is_empty());
        assert_eq!(due_ids(&db, 160), ["final", "open"]);
        assert_eq!(due_ids(&db, 1900), ["closed", "final", "open"]);
        db.execute("UPDATE assignments SET lifecycle='retired'", []).unwrap();
        assert_eq!(due_ids(&db, 160), ["open"]);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn assignment_facts_and_repository_rotation() {
        let db = database(Connection::open_in_memory().unwrap());
        db.execute_batch("INSERT INTO repositories VALUES('other','o/other');
            INSERT INTO work_items(node_id,repository_node_id,kind,number,state) VALUES('other','other','issue',4,'CLOSED');
            INSERT INTO assignments VALUES('closed','materializing');").unwrap();
        for id in ["open", "closed", "final", "other"] {
            record(&db, id, 100, true).unwrap();
        }
        assert_eq!(due_ids(&db, 160), ["closed", "final", "open"]);
        db.execute(
            "UPDATE assignments SET lifecycle='active' WHERE work_item_node_id='closed'",
            [],
        )
        .unwrap();
        assert_eq!(due_ids(&db, 160), ["closed", "final", "open"]);
        db.execute(
            "UPDATE assignments SET lifecycle='sleeping' WHERE work_item_node_id='closed'",
            [],
        )
        .unwrap();
        assert_eq!(due_ids(&db, 160), ["final", "open"]);
        // Previously attempted repository cannot keep untouched cold history behind it.
        for id in ["open", "closed", "final"] {
            record(&db, id, 1900, false).unwrap();
        }
        let due = due(&db, 1960, 60).unwrap();
        assert_eq!(due[0].repository, "o/other");
        assert_eq!(due[0].number, 4);
        assert_eq!(due[0].kind, "issue");
        assert_eq!(due[0].state, "CLOSED");
    }

    #[test]
    fn failed_traversal_backs_off_without_losing_the_last_success() {
        let db = database(Connection::open_in_memory().unwrap());
        // This closed item has never succeeded. Its failure cannot delay other
        // items, start the cold interval, or retry before the hot interval.
        record(&db, "closed", 100, false).unwrap();
        assert_eq!(due_ids(&db, 159), ["final", "open"]);
        assert_eq!(due_ids(&db, 160), ["closed", "final", "open"]);

        // After an open item succeeds and a refresh fails, closing it reveals
        // which persisted timestamp controls the cold interval.
        record(&db, "open", 100, true).unwrap();
        record(&db, "open", 160, false).unwrap();
        assert!(!due_ids(&db, 219).iter().any(|id| id == "open"));
        assert!(due_ids(&db, 220).iter().any(|id| id == "open"));
        db.execute("UPDATE work_items SET state='CLOSED' WHERE node_id='open'", []).unwrap();
        assert!(!due_ids(&db, 1899).iter().any(|id| id == "open"));
        assert!(due_ids(&db, 1900).iter().any(|id| id == "open"));
        record(&db, "open", 1900, false).unwrap();
        assert!(!due_ids(&db, 1959).iter().any(|id| id == "open"));
        assert!(due_ids(&db, 1960).iter().any(|id| id == "open"));
    }
}
