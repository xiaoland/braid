//! Existing-generation handoff facts; no activation or independent execution queue.
use super::{
    NewGhWriteIntent, PendingGitHubWrite, SchedulerPolicy, StoreError, gh_writes, now_rfc3339,
    open_read_only, publication_source, schedule_event,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::path::Path;

#[derive(serde::Deserialize)]
struct HandoffPolicy {
    quiet_seconds: u64,
    event_threshold: u32,
}

fn invalid(message: &str) -> StoreError {
    StoreError::InvalidData(message.into())
}

fn source_issue(
    connection: &Connection,
    source: &publication_source::PublicationSource,
) -> Result<String, StoreError> {
    publication_source::validate(connection, source)?;
    connection.query_row("SELECT i.node_id FROM assignments a JOIN work_items i ON i.node_id=a.work_item_node_id WHERE a.assignment_id=?1 AND a.lifecycle='active' AND i.kind='issue' AND lower(i.state)='open' AND ?2='issue_agent'", params![source.assignment_id, source.role], |r| r.get(0)).optional()?.ok_or_else(|| invalid("handoff requires an active Issue Agent on its OPEN Issue"))
}

fn target_assignment(
    connection: &Connection,
    repository: &str,
    target: &str,
) -> Result<String, StoreError> {
    connection
        .query_row(
            "SELECT a.assignment_id
         FROM assignments a
         JOIN work_items i ON i.node_id=a.work_item_node_id
         JOIN repositories r ON r.node_id=i.repository_node_id
         JOIN agent_instances ai ON ai.assignment_id=a.assignment_id
         WHERE r.name_with_owner=?1
         AND r.name_with_owner || '#' || i.number=?2
         AND i.kind='pr'
         AND lower(i.state)='open'
         AND a.lifecycle='active'
         AND ai.role='pr_implementation_agent'
         AND ai.lifecycle IN ('idle','running')
         AND ai.context_pressure NOT IN ('hard','unavailable')
         AND EXISTS (SELECT 1
         FROM provider_sessions ps
         WHERE ps.agent_id=ai.agent_id
         AND ps.lifecycle IN ('idle','running')
         AND NOT EXISTS (SELECT 1 FROM provider_sessions newer_session WHERE newer_session.agent_id=ps.agent_id AND newer_session.rowid>ps.rowid))
         AND NOT EXISTS (SELECT 1
         FROM assignments newer
         WHERE newer.work_item_node_id=i.node_id
         AND newer.generation>a.generation)",
            params![repository, target],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| invalid("handoff target needs a current active schedulable PR generation"))
}

fn published_design(connection: &Connection, issue: &str) -> Result<(), StoreError> {
    let pending: bool = connection.query_row("SELECT EXISTS(SELECT 1
         FROM write_intents w
         JOIN work_items i ON i.node_id=?1
         JOIN repositories r ON r.node_id=i.repository_node_id
         WHERE w.operation='body_edit'
         AND w.lifecycle IN ('pending','sending','uncertain','ambiguous')
         AND w.repository=r.name_with_owner
         AND (w.target=r.name_with_owner || '#' || i.number OR w.target='https://github.com/' || r.name_with_owner || '/issues/' || i.number))", [issue], |r| r.get(0))?;
    if pending {
        return Err(invalid(
            "Issue design edit is not confirmed applied; query its receipt before handoff",
        ));
    }
    Ok(())
}

pub(super) fn bind(
    tx: &Transaction<'_>,
    intent: &str,
    write: &NewGhWriteIntent,
) -> Result<(), StoreError> {
    let source =
        write.source.as_ref().ok_or_else(|| invalid("handoff requires a bound Issue Agent"))?;
    let issue = source_issue(tx, source)?;
    published_design(tx, &issue)?;
    let assignment = target_assignment(tx, &write.repository, &write.target)?;
    tx.execute(
        "INSERT INTO pr_handoffs(intent_id,target_assignment_id) VALUES (?1,?2)",
        params![intent, assignment],
    )?;
    Ok(())
}

pub(crate) fn validate(database: &Path, receipt: &super::GhWriteReceipt) -> Result<(), StoreError> {
    validate_on(&open_read_only(database)?, receipt).map(|_| ())
}

fn validate_on(
    connection: &Connection,
    receipt: &super::GhWriteReceipt,
) -> Result<String, StoreError> {
    let source = receipt.source.as_ref().ok_or_else(|| invalid("handoff requires source"))?;
    let issue = source_issue(connection, source)?;
    published_design(connection, &issue)?;
    let current = target_assignment(connection, &receipt.repository, &receipt.target)?;
    let bound: String = connection.query_row(
        "SELECT target_assignment_id FROM pr_handoffs WHERE intent_id=?1",
        [&receipt.intent_id],
        |r| r.get(0),
    )?;
    if bound != current {
        return Err(invalid("handoff target generation changed; request cannot be redirected"));
    }
    Ok(bound)
}

fn deliver_if_handoff(tx: &Transaction<'_>, write: &PendingGitHubWrite) -> Result<(), StoreError> {
    let receipt = gh_writes::gh_write_receipt_on(tx, &write.intent_id)?
        .ok_or_else(|| invalid("missing receipt"))?;
    if receipt.operation != "pr_handoff" {
        return Ok(());
    }
    let assignment = validate_on(tx, &receipt)?;
    let existing: Option<String> = tx.query_row(
        "SELECT event_id FROM pr_handoffs WHERE intent_id=?1",
        [&write.intent_id],
        |r| r.get(0),
    )?;
    if existing.is_some() {
        return Ok(());
    }
    let item: String = tx.query_row(
        "SELECT work_item_node_id FROM assignments WHERE assignment_id=?1",
        [&assignment],
        |r| r.get(0),
    )?;
    let event = uuid::Uuid::now_v7().to_string();
    let now = now_rfc3339();
    let source = receipt.source.as_ref().ok_or_else(|| invalid("missing source"))?;
    let reference: String = tx.query_row("SELECT r.name_with_owner || '#' || i.number FROM assignments a JOIN work_items i ON i.node_id=a.work_item_node_id JOIN repositories r ON r.node_id=i.repository_node_id WHERE a.assignment_id=?1", [&source.assignment_id], |r| r.get(0))?;
    tx.execute("INSERT INTO events(event_id,work_item_node_id,kind,detail,origin,reference,lifecycle,observed_at,dedupe_key) VALUES (?1,?2,'wake','pr_handoff','internal',?3,'pending',?4,?5)", params![event, item, format!("Explicit Issue handoff: {reference}; read current canonical design and discussion"), now, format!("pr_handoff:{}", write.intent_id)])?;
    // Policy is frozen in the accepted request, not a separate workflow.
    let HandoffPolicy { quiet_seconds, event_threshold } =
        serde_json::from_str(&receipt.payload).map_err(|e| invalid(&e.to_string()))?;
    schedule_event(
        tx,
        &item,
        &event,
        SchedulerPolicy { quiet_seconds, event_threshold },
        false,
        &now,
    )?;
    tx.execute(
        "UPDATE pr_handoffs SET event_id=?2 WHERE intent_id=?1",
        params![write.intent_id, event],
    )?;
    Ok(())
}

pub(crate) fn batch_needs_refresh(database: &Path, batch: &str) -> Result<bool, StoreError> {
    Ok(open_read_only(database)?.query_row("SELECT EXISTS(SELECT 1 FROM pr_handoffs h JOIN wake_batch_events be ON be.event_id=h.event_id WHERE be.batch_id=?1)", [batch], |r| r.get(0))?)
}

/// Canonical invalidity retires only this historical Wake, never its applied receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetirementReason {
    TargetGenerationChanged,
    TargetClosed,
    SourceNotOpenAssociated,
}

#[derive(Debug, Clone)]
pub(crate) struct RetiredInput {
    pub event_id: String,
    pub reason: RetirementReason,
}

pub(crate) struct BatchSource {
    pub event_id: String,
    pub repository: String,
    pub number: u64,
    pub current_generation: bool,
}

/// Post-delivery source authority is historical: do not revalidate the source
/// turn/assignment fence. Only canonical association/OPEN state still matters.
pub(crate) fn batch_sources(database: &Path, batch: &str) -> Result<Vec<BatchSource>, StoreError> {
    let connection = open_read_only(database)?;
    let mut statement = connection.prepare("SELECT h.event_id,w.repository,i.number,
        NOT EXISTS (SELECT 1 FROM assignments newer WHERE newer.work_item_node_id=a.work_item_node_id AND newer.generation>a.generation)
        AND EXISTS (SELECT 1 FROM turns t JOIN provider_sessions ps ON ps.session_id=t.session_id JOIN agent_instances ai ON ai.agent_id=ps.agent_id WHERE t.batch_id=be.batch_id AND t.lifecycle='starting' AND ai.assignment_id=a.assignment_id)
        FROM pr_handoffs h JOIN write_intents w ON w.intent_id=h.intent_id
        JOIN assignments a ON a.assignment_id=h.target_assignment_id
        JOIN assignments source ON source.assignment_id=json_extract(w.source_binding,'$.assignment_id')
        JOIN work_items i ON i.node_id=source.work_item_node_id
        JOIN wake_batch_events be ON be.event_id=h.event_id WHERE be.batch_id=?1")?;
    let rows = statement.query_map([batch], |r| {
        Ok(BatchSource {
            event_id: r.get(0)?,
            repository: r.get(1)?,
            number: super::sqlite_i64_to_u64(r.get(2)?, "handoff source number")?,
            current_generation: r.get(3)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(StoreError::from)
}

/// Delivery and receipt settlement share the caller's outbox attempt transaction.
pub(super) fn settle(
    tx: &Transaction<'_>,
    write: &PendingGitHubWrite,
) -> Result<Option<String>, StoreError> {
    tx.execute_batch("SAVEPOINT handoff_delivery")?;
    match deliver_if_handoff(tx, write) {
        Ok(()) => {
            tx.execute_batch("RELEASE handoff_delivery")?;
            Ok(None)
        }
        Err(StoreError::InvalidData(message)) => {
            tx.execute_batch("ROLLBACK TO handoff_delivery; RELEASE handoff_delivery")?;
            Ok(Some(message))
        }
        Err(error) => Err(error),
    }
}
