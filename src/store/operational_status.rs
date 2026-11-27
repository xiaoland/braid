//! Durable desired projections, not assignment or execution recovery.
use super::{
    StoreError, configure_connection, now_rfc3339, open_read_write, require_current_schema,
};
use rusqlite::{OptionalExtension as _, params};
use sha2::{Digest as _, Sha256};
use std::path::Path;
use uuid::Uuid;

struct Key {
    node: String,
    profile: String,
    generation: i64,
}

pub(super) fn enqueue_failure(
    database: &Path,
    assignment: &str,
    body: &str,
    observed_at: Option<&str>,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if body.trim().is_empty() {
        return Err(StoreError::InvalidData("Operational Status body is empty".into()));
    }
    let now = now_rfc3339();
    let mut db = open_read_write(database)?;
    configure_connection(&db)?;
    let tx = db.transaction()?;
    let (key,repository,number)=tx.query_row("SELECT w.node_id,g.profile_id,a.generation,r.name_with_owner,w.number FROM assignments a JOIN agent_instances g ON g.assignment_id=a.assignment_id JOIN work_items w ON w.node_id=a.work_item_node_id JOIN repositories r ON r.node_id=w.repository_node_id WHERE a.assignment_id=?1",[assignment],|r|Ok((Key{node:r.get(0)?,profile:r.get(1)?,generation:r.get(2)?},r.get::<_,String>(3)?,r.get::<_,i64>(4)?)))?;
    let existing=tx.query_row("SELECT projection_kind,failure_body FROM status_comments WHERE work_item_node_id=?1 AND profile_id=?2 AND assignment_generation=?3",params![key.node,key.profile,key.generation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).optional()?;
    if existing.as_ref().is_some_and(|(kind, previous)| {
        kind == "superseded" || (kind == "failure" && previous.as_deref() == Some(body))
    }) {
        return Ok(());
    }
    let observed = observed_at.unwrap_or(&now);
    tx.execute("INSERT INTO status_comments(work_item_node_id,profile_id,assignment_generation,body_digest,lifecycle,updated_at,projection_kind,failure_body,failure_recorded_at) VALUES(?1,?2,?3,?4,'pending',?5,'failure',?6,?7) ON CONFLICT(work_item_node_id,profile_id,assignment_generation) DO UPDATE SET projection_kind='failure',failure_body=excluded.failure_body,failure_recorded_at=excluded.failure_recorded_at,successor_generation=NULL",params![key.node,key.profile,key.generation,"0".repeat(64),now,body,observed])?;
    let rendered = format!(
        "{body}\n\n---\n所属 generation：**{}**；故障确认时间：`{observed}`。此条不是其他 generation 的当前状态。",
        key.generation
    );
    publish(&tx, &key, &repository, number, &rendered, &now)?;
    tx.commit()?;
    Ok(())
}

// One outbox claim owns reconciliation, including after a service restart.
// Only an accepted current turn or its confirmed normal terminal is evidence;
// assignment creation, a clean worktree or a memory reset is not recovery.
pub(super) fn reconcile(tx: &rusqlite::Transaction<'_>, now: &str) -> Result<(), StoreError> {
    let mut query=tx.prepare("SELECT s.work_item_node_id,s.profile_id,s.assignment_generation,r.name_with_owner,w.number,s.failure_body,s.failure_recorded_at,a.generation,g.profile_id,p.lifecycle,t.lifecycle,CASE WHEN t.lifecycle='completed' THEN t.ended_at ELSE t.started_at END FROM status_comments s JOIN work_items w ON w.node_id=s.work_item_node_id JOIN repositories r ON r.node_id=w.repository_node_id JOIN assignments a ON a.work_item_node_id=w.node_id AND a.generation=(SELECT max(newest.generation) FROM assignments newest WHERE newest.work_item_node_id=w.node_id) JOIN agent_instances g ON g.assignment_id=a.assignment_id JOIN provider_sessions p ON p.agent_id=g.agent_id AND p.rowid=(SELECT max(latest.rowid) FROM provider_sessions latest WHERE latest.agent_id=g.agent_id) JOIN turns t ON t.session_id=p.session_id AND t.rowid=(SELECT max(last.rowid) FROM turns last WHERE last.session_id=p.session_id) WHERE a.lifecycle='active' AND g.lifecycle!='blocked' AND t.provider_turn_id IS NOT NULL AND ((p.lifecycle='idle' AND t.lifecycle='completed') OR (p.lifecycle='running' AND t.lifecycle='running')) AND (s.projection_kind='failure' OR (s.assignment_generation<a.generation AND COALESCE(s.successor_generation,s.assignment_generation)<a.generation))")?;
    let rows = query
        .query_map([], |r| {
            Ok(Recovery {
                key: Key { node: r.get(0)?, profile: r.get(1)?, generation: r.get(2)? },
                repository: r.get(3)?,
                number: r.get(4)?,
                failure: r.get(5)?,
                failure_at: r.get(6)?,
                generation: r.get(7)?,
                profile: r.get(8)?,
                session: r.get(9)?,
                turn: r.get(10)?,
                confirmed: r.get(11)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for row in rows {
        let Some(confirmed) = row.confirmed.as_deref() else { continue };
        if row.generation == row.key.generation
            && !confirmation_after(confirmed, row.failure_at.as_deref())?
        {
            continue;
        }
        let successor = row.generation > row.key.generation;
        let label = if successor {
            format!("历史故障 · 已由 generation {} 接替", row.generation)
        } else {
            "执行可用性已恢复".into()
        };
        let previous = row.failure.as_deref().unwrap_or("原故障正文未在本地留存。");
        let quoted =
            previous.lines().map(|line| format!("> {line}")).collect::<Vec<_>>().join("\n");
        let body = format!(
            "> **Braid Operational Status · `{}` · generation {}**\n\n**{label}**\n\n最后确认：`{confirmed}`。generation **{}**（Profile `{}`）已由 Provider 接受执行；该次确认的 Session 状态为 `{}`，turn 状态为 `{}`。\n\n{}这只确认执行接手或正常终止，不代表代码已修改、review 已落实或产品已验收，也不是未来持续可用的保证。\n\n### 原故障（历史记录）\n\n{quoted}",
            row.key.profile,
            row.key.generation,
            row.generation,
            row.profile,
            row.session,
            row.turn,
            if successor { "原 generation 未被解阻或复用。" } else { "" }
        );
        publish(tx, &row.key, &row.repository, row.number, &body, now)?;
        tx.execute("UPDATE status_comments SET projection_kind=?4,successor_generation=?5 WHERE work_item_node_id=?1 AND profile_id=?2 AND assignment_generation=?3",params![row.key.node,row.key.profile,row.key.generation,if successor{"superseded"}else{"recovered"},successor.then_some(row.generation)])?;
    }
    // A preceding create may have settled since this desired projection was
    // queued. Resolve the stable remote ID at claim, never create a second one.
    tx.execute("UPDATE github_write_outbox AS o SET operation='comment_update',target_kind='issue_comment',target_database_id=(SELECT s.remote_comment_database_id FROM status_comments s WHERE s.work_item_node_id=o.status_work_item_node_id AND s.profile_id=o.status_profile_id AND s.assignment_generation=o.status_generation) WHERE o.lifecycle='pending' AND o.operation='comment_create' AND EXISTS(SELECT 1 FROM status_comments s WHERE s.work_item_node_id=o.status_work_item_node_id AND s.profile_id=o.status_profile_id AND s.assignment_generation=o.status_generation AND s.remote_comment_database_id IS NOT NULL AND s.remote_comment_node_id IS NOT NULL)",[])?;
    block_unidentified_creates(tx, now)?;
    Ok(())
}

/// A terminal multiple-match receipt cannot identify the original comment.
/// Propagate that uncertainty to later creates for this key rather than
/// guessing a PATCH target, issuing another POST, or stalling other keys.
fn block_unidentified_creates(tx: &rusqlite::Transaction<'_>, now: &str) -> Result<(), StoreError> {
    let blocked = tx.execute(
        "UPDATE github_write_outbox AS desired
         SET lifecycle='ambiguous', updated_at=?1,
             last_error='Earlier status create matched multiple App comments; remote identity unresolved; no new create attempted'
         WHERE desired.status_work_item_node_id IS NOT NULL
           AND desired.operation='comment_create' AND desired.lifecycle='pending'
           AND EXISTS (
             SELECT 1 FROM github_write_outbox AS earlier
             WHERE earlier.status_work_item_node_id=desired.status_work_item_node_id
               AND earlier.status_profile_id=desired.status_profile_id
               AND earlier.status_generation=desired.status_generation
               AND earlier.rowid<desired.rowid AND earlier.operation='comment_create'
               AND earlier.lifecycle='ambiguous')",
        [now],
    )?;
    if blocked != 0 {
        tx.execute(
            "UPDATE status_comments SET lifecycle='ambiguous',updated_at=?1
                    WHERE lifecycle='pending' AND write_intent_id IN
                      (SELECT intent_id FROM github_write_outbox WHERE lifecycle='ambiguous'
                       AND status_work_item_node_id IS NOT NULL)",
            [now],
        )?;
    }
    Ok(())
}

pub(super) fn confirmation_after(
    confirmed: &str,
    failure: Option<&str>,
) -> Result<bool, StoreError> {
    let Some(failure) = failure else { return Ok(false) };
    let parse = |value: &str| {
        time::OffsetDateTime::parse(value, &time::format_description::well_known::Rfc3339).map_err(
            |error| {
                StoreError::InvalidData(format!(
                    "invalid operational status confirmation timestamp: {error}"
                ))
            },
        )
    };
    Ok(parse(confirmed)? > parse(failure)?)
}

struct Recovery {
    key: Key,
    repository: String,
    number: i64,
    failure: Option<String>,
    failure_at: Option<String>,
    generation: i64,
    profile: String,
    session: String,
    turn: String,
    confirmed: Option<String>,
}

fn publish(
    tx: &rusqlite::Transaction<'_>,
    key: &Key,
    repository: &str,
    number: i64,
    body: &str,
    now: &str,
) -> Result<(), StoreError> {
    let (old_digest,old_intent,remote)=tx.query_row("SELECT body_digest,write_intent_id,remote_comment_database_id FROM status_comments WHERE work_item_node_id=?1 AND profile_id=?2 AND assignment_generation=?3",params![key.node,key.profile,key.generation],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)))?;
    let digest = hex::encode(Sha256::digest(body.as_bytes()));
    if digest == old_digest && old_intent.is_some() {
        return Ok(());
    }
    let intent = Uuid::now_v7().to_string();
    tx.execute("UPDATE github_write_outbox SET lifecycle='superseded',updated_at=?4 WHERE status_work_item_node_id=?1 AND status_profile_id=?2 AND status_generation=?3 AND lifecycle='pending'",params![key.node,key.profile,key.generation,now])?;
    let request =
        hex::encode(Sha256::digest(format!("operational-status\0{intent}\0{digest}").as_bytes()));
    let (operation, kind, target) = remote.map_or_else(
        || ("comment_create", "issue", number.to_string()),
        |id| ("comment_update", "issue_comment", id),
    );
    tx.execute("INSERT INTO github_write_outbox(intent_id,repository,target_kind,target_database_id,operation,content,request_digest,lifecycle,next_attempt_at,created_at,updated_at,status_work_item_node_id,status_profile_id,status_generation) VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8,?8,?8,?9,?10,?11)",params![intent,repository,kind,target,operation,body,request,now,key.node,key.profile,key.generation])?;
    tx.execute("UPDATE status_comments SET write_intent_id=?4,body_digest=?5,lifecycle='pending',updated_at=?6 WHERE work_item_node_id=?1 AND profile_id=?2 AND assignment_generation=?3",params![key.node,key.profile,key.generation,intent,digest,now])?;
    Ok(())
}

pub(super) fn settle(
    tx: &rusqlite::Transaction<'_>,
    intent: &str,
    lifecycle: &str,
    remote_id: Option<&str>,
    remote_node: Option<&str>,
    now: &str,
) -> Result<(), StoreError> {
    // A stale publication still owns its remote create result, but not the
    // lifecycle of a newer desired publication. Keep both facts atomically.
    tx.execute("UPDATE status_comments SET remote_comment_node_id=COALESCE(?3,remote_comment_node_id),remote_comment_database_id=COALESCE(?4,remote_comment_database_id),lifecycle=CASE WHEN write_intent_id=?1 THEN ?2 ELSE lifecycle END,updated_at=CASE WHEN write_intent_id=?1 THEN ?5 ELSE updated_at END WHERE EXISTS(SELECT 1 FROM github_write_outbox o WHERE o.intent_id=?1 AND o.status_work_item_node_id=status_comments.work_item_node_id AND o.status_profile_id=status_comments.profile_id AND o.status_generation=status_comments.assignment_generation)",params![intent,lifecycle,remote_node,remote_id,now])?;
    Ok(())
}
