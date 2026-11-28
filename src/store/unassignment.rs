use super::{StoreError, open_read_only, require_current_schema};
use rusqlite::{OptionalExtension as _, params};
use std::path::Path;

pub(crate) enum UnassignmentTarget {
    Event(String),
    RunningTurn(String),
}

pub(crate) struct Unassignment {
    pub event_id: String,
    pub(super) assignment_id: Option<String>,
    pub repository: String,
    pub number: u64,
    pub sessions: Vec<(String, bool)>,
}

/// An active worker only handles the revocation of its own work item.
/// Idle workers preserve the normal order of assignment candidates.
pub(super) fn due(
    database: &Path,
    target: &UnassignmentTarget,
    quiet_seconds: u64,
) -> Result<Option<Unassignment>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let (event, turn) = match target {
        UnassignmentTarget::Event(id) => (Some(id), None),
        UnassignmentTarget::RunningTurn(id) => (None, Some(id)),
    };
    let row = connection.query_row(
        "SELECT e.event_id,r.name_with_owner,w.number,
                (SELECT latest.observed_at FROM events latest WHERE latest.work_item_node_id=w.node_id
                   AND latest.lifecycle='pending' AND latest.kind='unassign' ORDER BY latest.rowid DESC LIMIT 1),
                (SELECT a.assignment_id FROM assignments a WHERE a.work_item_node_id=w.node_id
                   AND a.lifecycle IN ('materializing','active','finalizing') ORDER BY a.generation DESC LIMIT 1)
         FROM events e JOIN work_items w ON w.node_id=e.work_item_node_id
         JOIN repositories r ON r.node_id=w.repository_node_id
         WHERE e.kind='unassign' AND e.lifecycle='pending' AND (?1 IS NULL OR e.event_id=?1)
           AND e.work_item_node_id=coalesce((SELECT work_item_node_id FROM events WHERE event_id=?1),
             (SELECT a.work_item_node_id FROM turns t JOIN provider_sessions ps ON ps.session_id=t.session_id
              JOIN agent_instances ai ON ai.agent_id=ps.agent_id JOIN assignments a ON a.assignment_id=ai.assignment_id
              WHERE t.turn_id=?2 AND t.lifecycle IN ('starting','running')))
         ORDER BY e.rowid LIMIT 1",
        params![event, turn],
        |row| Ok((
            row.get::<_, String>(0)?, row.get::<_,String>(1)?,
            super::sqlite_i64_to_u64(row.get(2)?, "unassignment Work Item number")?,
            row.get::<_,String>(3)?, row.get::<_,Option<String>>(4)?,
        )),
    ).optional()?;
    let Some((event_id, repository, number, latest, assignment_id)) = row else { return Ok(None) };
    let observed =
        time::OffsetDateTime::parse(&latest, &time::format_description::well_known::Rfc3339)
            .map_err(|error| {
                StoreError::InvalidData(format!("unassignment observed_at: {error}"))
            })?;
    if time::OffsetDateTime::now_utc() - observed
        < time::Duration::seconds(i64::try_from(quiet_seconds).unwrap_or(i64::MAX))
    {
        return Ok(None);
    }
    let mut statement = connection.prepare(
        "SELECT ps.provider_session_id, ps.execution_stopped_at IS NOT NULL
         FROM provider_sessions ps JOIN agent_instances ai ON ai.agent_id=ps.agent_id
         WHERE ai.assignment_id=?1 AND ps.lifecycle NOT IN ('retired','replaced')",
    )?;
    let sessions = statement
        .query_map([&assignment_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(Unassignment { event_id, assignment_id, repository, number, sessions }))
}
