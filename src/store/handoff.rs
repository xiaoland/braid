//! Existing-generation handoff facts; no activation or independent execution queue.
use super::{
    NewGhWriteIntent, PendingGitHubWrite, SchedulerPolicy, StoreError, gh_writes, handoff_inputs,
    open_read_only, publication_source,
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

struct SourceIssue {
    node_id: String,
    number: i64,
}

fn source_issue(
    connection: &Connection,
    source: &publication_source::PublicationSource,
    repository: &str,
) -> Result<SourceIssue, StoreError> {
    publication_source::validate(connection, source)?;
    connection
        .query_row(
            "SELECT i.node_id,i.number FROM assignments a
             JOIN work_items i ON i.node_id=a.work_item_node_id
             JOIN repositories r ON r.node_id=i.repository_node_id
             WHERE a.assignment_id=?1 AND a.lifecycle='active'
               AND i.kind='issue' AND lower(i.state)='open'
               AND ?2='issue_agent' AND r.name_with_owner=?3",
            params![source.assignment_id, source.role, repository],
            |r| Ok(SourceIssue { node_id: r.get(0)?, number: r.get(1)? }),
        )
        .optional()?
        .ok_or_else(|| invalid("handoff requires an active Issue Agent on its OPEN Issue"))
}

struct TargetAssignment {
    assignment_id: String,
    work_item_node_id: String,
    generation: i64,
}

fn target_assignment(
    connection: &Connection,
    repository: &str,
    target: &str,
) -> Result<TargetAssignment, StoreError> {
    connection
        .query_row(
            "SELECT a.assignment_id,i.node_id,a.generation
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
            |r| Ok(TargetAssignment {
                assignment_id: r.get(0)?,
                work_item_node_id: r.get(1)?,
                generation: r.get(2)?,
            }),
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
    let issue = source_issue(tx, source, &write.repository)?;
    published_design(tx, &issue.node_id)?;
    let assignment = target_assignment(tx, &write.repository, &write.target)?;
    tx.execute(
        "INSERT INTO pr_handoffs(intent_id,target_assignment_id) VALUES (?1,?2)",
        params![intent, assignment.assignment_id],
    )?;
    Ok(())
}

pub(crate) fn validate(database: &Path, receipt: &super::GhWriteReceipt) -> Result<(), StoreError> {
    validate_on(&open_read_only(database)?, receipt).map(|_| ())
}

fn validate_on(
    connection: &Connection,
    receipt: &super::GhWriteReceipt,
) -> Result<Option<(SourceIssue, TargetAssignment)>, StoreError> {
    let bound: Option<String> = connection.query_row(
        "SELECT target_assignment_id FROM pr_handoffs WHERE intent_id=?1",
        [&receipt.intent_id],
        |r| r.get(0),
    )?;
    let Some(bound) = bound else {
        return Ok(None);
    };
    let source = receipt.source.as_ref().ok_or_else(|| invalid("handoff requires source"))?;
    let issue = source_issue(connection, source, &receipt.repository)?;
    published_design(connection, &issue.node_id)?;
    let current = target_assignment(connection, &receipt.repository, &receipt.target)?;
    if bound != current.assignment_id {
        return Err(invalid("handoff target generation changed; request cannot be redirected"));
    }
    Ok(Some((issue, current)))
}

fn deliver_if_handoff(tx: &Transaction<'_>, write: &PendingGitHubWrite) -> Result<(), StoreError> {
    let receipt = gh_writes::gh_write_receipt_on(tx, &write.intent_id)?
        .ok_or_else(|| invalid("missing receipt"))?;
    if receipt.operation != "pr_handoff" {
        return Ok(());
    }
    let Some((source, target)) = validate_on(tx, &receipt)? else {
        return Ok(());
    };
    let HandoffPolicy { quiet_seconds, event_threshold } =
        serde_json::from_str(&receipt.payload).map_err(|e| invalid(&e.to_string()))?;
    let event = handoff_inputs::accept(
        tx,
        handoff_inputs::NewHandoffInput {
            source_repository: &receipt.repository,
            source_number: source.number,
            target_work_item: &target.work_item_node_id,
            target_assignment: &target.assignment_id,
            target_generation: target.generation,
        },
        SchedulerPolicy { quiet_seconds, event_threshold },
    )?;
    tx.execute(
        "UPDATE pr_handoffs SET event_id=?2,target_assignment_id=NULL WHERE intent_id=?1",
        params![write.intent_id, event],
    )?;
    Ok(())
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
