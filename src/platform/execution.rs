//! Platform evidence that an execution attempt can no longer acquire or exercise write authority.
use std::path::Path;

use anyhow::{Result, ensure};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct FenceExecution {
    pub client_id: String,
    pub execution_id: String,
}

pub(crate) fn fence(database: &Path, request: FenceExecution) -> Result<FenceExecution> {
    ensure!(!request.client_id.is_empty(), "execution client must not be empty");
    require_canonical_id(&request.execution_id)?;
    let connection = Connection::open(database)?;
    connection.busy_timeout(std::time::Duration::from_secs(5))?;
    connection.execute(
        "INSERT OR IGNORE INTO execution_fences(client_id,execution_id) VALUES(?1,?2)",
        params![request.client_id, request.execution_id],
    )?;
    Ok(request)
}

pub(crate) fn require_open(
    connection: &Connection,
    client_id: &str,
    execution_id: &str,
) -> Result<()> {
    require_canonical_id(execution_id)?;
    let fenced: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM execution_fences WHERE client_id=?1 AND execution_id=?2)",
        params![client_id, execution_id],
        |row| row.get(0),
    )?;
    ensure!(!fenced, "execution attempt is fenced");
    Ok(())
}

fn require_canonical_id(value: &str) -> Result<()> {
    ensure!(
        uuid::Uuid::parse_str(value)?.to_string() == value,
        "execution identity must be a canonical UUID"
    );
    Ok(())
}
