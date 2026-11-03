//! Durable execution identity and result; collaboration objects remain upstream-owned.
use super::{StoreError, configure_connection, open_read_write, require_current_schema};
use rusqlite::OptionalExtension;

pub enum RunCommand {
    Read(String),
    Save { run_id: String, request: String, result: String },
}

pub(super) fn execute(
    database: &std::path::Path,
    command: RunCommand,
) -> Result<Option<String>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    match command {
        RunCommand::Read(id) => Ok(connection
            .query_row("SELECT result_json FROM harness_runs WHERE run_id=?1", [id], |row| {
                row.get(0)
            })
            .optional()?),
        RunCommand::Save { run_id, request, result } => {
            let existing: Option<String> = connection
                .query_row(
                    "SELECT request_json FROM harness_runs WHERE run_id=?1",
                    [&run_id],
                    |row| row.get(0),
                )
                .optional()?;
            if existing.as_ref().is_some_and(|value| value != &request) {
                return Err(StoreError::InvalidData(
                    "run identity is bound to a different input or execution recipe".into(),
                ));
            }
            connection.execute("INSERT INTO harness_runs(run_id,request_json,result_json) VALUES(?1,?2,?3) ON CONFLICT(run_id) DO UPDATE SET result_json=excluded.result_json", rusqlite::params![run_id,request,result])?;
            Ok(None)
        }
    }
}
