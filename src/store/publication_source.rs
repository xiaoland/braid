//! Local provenance is resolved from durable worktree/session state, never an environment claim.
use super::{StoreError, open_read_only};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicationSource {
    pub agent_id: String,
    pub assignment_id: String,
    pub generation: i64,
    pub profile_id: String,
    pub role: String,
    pub profile_revision: i64,
    pub session_id: String,
    pub turn_id: String,
    pub context_revision: String,
}

pub fn resolve(
    database: &Path,
    cwd: &Path,
    profile: Option<&str>,
    agent_call: bool,
) -> Result<Option<PublicationSource>, StoreError> {
    let connection = open_read_only(database)?;
    let cwd = cwd.canonicalize().map_err(|e| StoreError::InvalidData(e.to_string()))?;
    // Prefer the current claimed turn even before native start is observed:
    // validate must reject an unobserved start, never borrow an older completed
    // turn's identity. Row insertion order breaks ties without nullable clocks.
    let mut statement = connection.prepare("SELECT w.path,a.agent_id,g.assignment_id,g.generation,a.profile_id,a.profile_revision,s.session_id,t.turn_id,t.context_revision,a.role FROM worktrees w JOIN agent_instances a ON a.agent_id=w.agent_id JOIN assignments g ON g.assignment_id=a.assignment_id JOIN provider_sessions s ON s.agent_id=a.agent_id JOIN turns t ON t.session_id=s.session_id ORDER BY g.generation DESC, s.rowid DESC, CASE t.lifecycle WHEN 'starting' THEN 0 WHEN 'running' THEN 0 ELSE 1 END, t.rowid DESC")?;
    let candidates = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            PublicationSource {
                agent_id: row.get(1)?,
                assignment_id: row.get(2)?,
                generation: row.get(3)?,
                profile_id: row.get(4)?,
                profile_revision: row.get(5)?,
                session_id: row.get(6)?,
                turn_id: row.get(7)?,
                context_revision: row.get(8)?,
                role: row.get(9)?,
            },
        ))
    })?;
    for candidate in candidates {
        let (path, source) = candidate?;
        if Path::new(&path).canonicalize().is_ok_and(|path| cwd.starts_with(path)) {
            if profile.is_some_and(|profile| profile != source.profile_id) {
                return Err(StoreError::InvalidData(
                    "publication Profile does not match source assignment".into(),
                ));
            }
            validate(&connection, &source)?;
            return Ok(Some(source));
        }
    }
    if agent_call {
        return Err(StoreError::InvalidData(
            "Agent publication requires a durable source worktree/session binding".into(),
        ));
    }
    Ok(None)
}

pub fn check(database: &Path, source: &PublicationSource) -> Result<(), StoreError> {
    validate(&open_read_only(database)?, source)
}

pub(super) fn validate(
    connection: &Connection,
    source: &PublicationSource,
) -> Result<(), StoreError> {
    // Completed turns keep their accepted obligations. Context replacement and generation
    // replacement do not. Session ID is the current available Context fence.
    let valid = connection
        .query_row(
            "SELECT 1 FROM agent_instances a
         JOIN assignments g ON g.assignment_id=a.assignment_id
         JOIN worktrees w ON w.agent_id=a.agent_id AND w.lifecycle='active'
         JOIN provider_sessions s ON s.agent_id=a.agent_id
         JOIN turns t ON t.session_id=s.session_id
         WHERE a.agent_id=?1 AND g.assignment_id=?2 AND g.generation=?3
           AND a.profile_id=?4 AND a.profile_revision=?5 AND a.role=?9 AND s.session_id=?6
           AND t.turn_id=?7 AND t.context_revision=?8 AND s.context_revision=?8
           AND t.lifecycle IN ('running','completed')
           AND s.lifecycle IN ('running','idle') AND g.lifecycle IN ('active','finalizing')
           AND NOT EXISTS (
             SELECT 1 FROM events e WHERE e.work_item_node_id=g.work_item_node_id
               AND e.lifecycle='pending' AND e.kind='invalidate' AND e.origin!='agent'
               AND (e.detail IS NOT 'cross_surface' OR EXISTS (
                 SELECT 1 FROM wake_batches wb JOIN wake_batch_events be ON be.batch_id=wb.batch_id
                 WHERE be.event_id=e.event_id AND wb.lifecycle='runnable')))
           AND NOT EXISTS (
             SELECT 1 FROM assignments newer WHERE newer.work_item_node_id=g.work_item_node_id
               AND newer.generation>g.generation)",
            params![
                source.agent_id,
                source.assignment_id,
                source.generation,
                source.profile_id,
                source.profile_revision,
                source.session_id,
                source.turn_id,
                source.context_revision,
                source.role
            ],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !valid {
        return Err(StoreError::InvalidData(
            "publication source was invalidated or superseded".into(),
        ));
    }
    Ok(())
}
