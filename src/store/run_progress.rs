//! Read-side facts used to control a bounded run and confirm its obligations.
use super::{
    OptionalExtension, Path, StoreError, open_read_only, require_current_schema, sqlite_i64_to_u64,
};

pub(crate) struct ExecutionProgress {
    pub(crate) blocked_group: Option<BlockedGroup>,
    pub(crate) blocked_reset: Option<String>,
    pub(crate) unresolved_reset: bool,
}

pub(crate) struct BlockedGroup {
    pub(crate) kind: String,
    pub(crate) number: u64,
    pub(crate) error: Option<String>,
}

pub(crate) struct PublicationProgress {
    pub(crate) pending: u64,
    pub(crate) uncertain: u64,
}

pub(super) fn execution(database: &Path) -> Result<ExecutionProgress, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let tx = connection.unchecked_transaction()?;
    let blocked_group = tx
        .query_row(
            "SELECT w.kind,w.number,ai.context_error
             FROM assignments a JOIN work_items w ON w.node_id=a.work_item_node_id
             JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
             LEFT JOIN provider_sessions ps ON ps.agent_id=ai.agent_id
             WHERE a.lifecycle='blocked' OR ps.lifecycle='blocked'
             ORDER BY a.assigned_at,a.assignment_id,ps.started_at LIMIT 1",
            [],
            |row| {
                Ok(BlockedGroup {
                    kind: row.get(0)?,
                    number: sqlite_i64_to_u64(row.get(1)?, "agent Work Item number")?,
                    error: row.get(2)?,
                })
            },
        )
        .optional()?;
    let blocked_reset = tx
        .query_row(
            "SELECT reset_id FROM context_resets WHERE lifecycle='blocked'
             ORDER BY created_at,reset_id LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let unresolved_reset = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM context_resets WHERE lifecycle!='applied')",
        [],
        |row| row.get(0),
    )?;
    tx.commit()?;
    Ok(ExecutionProgress { blocked_group, blocked_reset, unresolved_reset })
}

pub(super) fn publication(database: &Path) -> Result<PublicationProgress, StoreError> {
    require_current_schema(database)?;
    publication_on(&open_read_only(database)?)
}

pub(super) fn publication_on(
    connection: &rusqlite::Connection,
) -> Result<PublicationProgress, StoreError> {
    Ok(connection.query_row(
        "SELECT COUNT(*) FILTER (WHERE lifecycle IN ('pending','sending'))
                  + (SELECT COUNT(*) FROM github_reaction_feedback WHERE projected=0)
                  + (SELECT COUNT(*) FROM operational_status_facts),
                COUNT(*) FILTER (WHERE lifecycle='uncertain')
         FROM github_write_outbox WHERE lifecycle IN ('pending','sending','uncertain')",
        [],
        |row| {
            Ok(PublicationProgress {
                pending: sqlite_i64_to_u64(row.get(0)?, "pending writes")?,
                uncertain: sqlite_i64_to_u64(row.get(1)?, "uncertain writes")?,
            })
        },
    )?)
}
