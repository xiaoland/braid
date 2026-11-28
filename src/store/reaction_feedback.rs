//! GitHub feedback requested by accepted inputs and observed turn transitions.
use rusqlite::{OptionalExtension as _, Transaction, params};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;

use super::StoreError;

pub(super) fn record_eyes(
    tx: &Transaction<'_>,
    event_id: &str,
    repository: &str,
    target_kind: &str,
    target_database_id: &str,
    now: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO github_reaction_feedback
            (event_id,repository,target_kind,target_database_id,content,created_at)
         VALUES (?1,?2,?3,?4,'eyes',?5)",
        params![event_id, repository, target_kind, target_database_id, now],
    )?;
    Ok(())
}

pub(super) fn record_turn(
    tx: &Transaction<'_>,
    turn_id: &str,
    content: &str,
    now: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO github_reaction_feedback
            (turn_id,repository,target_kind,target_database_id,content,created_at)
         SELECT t.turn_id,f.repository,f.target_kind,f.target_database_id,?2,?3
         FROM turns t
         JOIN wake_batch_events be ON be.batch_id=t.batch_id
         JOIN events e ON e.event_id=be.event_id AND e.trusted_mention=1
         JOIN github_reaction_feedback f ON f.event_id=e.event_id
         WHERE t.turn_id=?1 ORDER BY be.ordinal LIMIT 1
         ON CONFLICT(turn_id,content) DO NOTHING",
        params![turn_id, content, now],
    )?;
    Ok(())
}

pub(super) fn project(tx: &Transaction<'_>) -> Result<(), StoreError> {
    let pending = {
        let mut query = tx.prepare(
            "SELECT sequence,event_id,repository,target_kind,target_database_id,content,created_at
             FROM github_reaction_feedback WHERE projected=0 ORDER BY sequence",
        )?;
        query
            .query_map([], |row| {
                Ok(Feedback {
                    sequence: row.get(0)?,
                    event_id: row.get(1)?,
                    repository: row.get(2)?,
                    target_kind: row.get(3)?,
                    target_database_id: row.get(4)?,
                    content: row.get(5)?,
                    created_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?
    };
    for feedback in pending {
        if matches!(feedback.content.as_str(), "clear_rocket" | "+1" | "confused") {
            enqueue_rocket_removal(
                tx,
                &feedback.repository,
                &feedback.target_kind,
                &feedback.target_database_id,
                &feedback.created_at,
            )?;
        }
        let intent =
            if feedback.content == "clear_rocket" { None } else { project_add(tx, &feedback)? };
        tx.execute(
            "UPDATE github_reaction_feedback SET projected=1,publication_intent_id=?2
             WHERE sequence=?1",
            params![feedback.sequence, intent],
        )?;
    }
    Ok(())
}

struct Feedback {
    sequence: i64,
    event_id: Option<String>,
    repository: String,
    target_kind: String,
    target_database_id: String,
    content: String,
    created_at: String,
}

fn project_add(tx: &Transaction<'_>, feedback: &Feedback) -> Result<Option<String>, StoreError> {
    let intent_id = Uuid::now_v7().to_string();
    let digest = hex::encode(Sha256::digest(
        format!(
            "{}\0{}\0{}\0{}",
            feedback.repository,
            feedback.target_kind,
            feedback.target_database_id,
            feedback.content
        )
        .as_bytes(),
    ));
    let inserted = tx.execute(
        "INSERT OR IGNORE INTO github_write_outbox
            (intent_id,event_id,repository,target_kind,target_database_id,operation,content,
             request_digest,lifecycle,next_attempt_at,created_at,updated_at)
         VALUES (?1,?2,?3,?4,?5,'reaction_add',?6,?7,'pending',?8,?8,?8)",
        params![
            intent_id,
            feedback.event_id,
            feedback.repository,
            feedback.target_kind,
            feedback.target_database_id,
            feedback.content,
            digest,
            feedback.created_at,
        ],
    )?;
    Ok((inserted == 1).then_some(intent_id))
}

pub(super) fn rocket_clear_requested_at(
    tx: &Transaction<'_>,
    intent_id: &str,
    repository: &str,
    target_kind: &str,
    target_database_id: &str,
) -> Result<Option<String>, StoreError> {
    Ok(tx.query_row(
        "SELECT COALESCE((
            SELECT created_at FROM github_reaction_feedback
            WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
              AND content IN ('clear_rocket','+1','confused')
              AND sequence>COALESCE((SELECT sequence FROM github_reaction_feedback
                                     WHERE publication_intent_id=?4),0)
            ORDER BY sequence LIMIT 1
         ), (
            SELECT o.created_at FROM github_write_outbox o
            WHERE o.repository=?1 AND o.target_kind=?2 AND o.target_database_id=?3
              AND o.operation='reaction_add' AND o.content IN ('+1','confused')
              AND o.lifecycle NOT IN ('rejected','conflict','ambiguous','superseded')
              AND NOT EXISTS (SELECT 1 FROM github_reaction_feedback f
                              WHERE f.publication_intent_id=o.intent_id)
            ORDER BY o.created_at LIMIT 1))",
        params![repository, target_kind, target_database_id, intent_id],
        |row| row.get(0),
    )?)
}

pub(super) fn enqueue_rocket_removal(
    tx: &Transaction<'_>,
    repository: &str,
    target_kind: &str,
    target_database_id: &str,
    now: &str,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE github_write_outbox SET lifecycle='superseded',updated_at=?4
         WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
           AND operation='reaction_add' AND content='rocket' AND lifecycle='pending'",
        params![repository, target_kind, target_database_id, now],
    )?;
    let remote = tx
        .query_row(
            "SELECT remote_database_id FROM github_write_outbox
             WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
               AND operation='reaction_add' AND content='rocket' AND lifecycle='applied'
               AND remote_database_id IS NOT NULL
             ORDER BY updated_at DESC LIMIT 1",
            params![repository, target_kind, target_database_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(remote_database_id) = remote else { return Ok(()) };
    let digest = hex::encode(Sha256::digest(
        format!(
            "{repository}\0{target_kind}\0{target_database_id}\0reaction_delete\0{remote_database_id}"
        )
        .as_bytes(),
    ));
    tx.execute(
        "INSERT OR IGNORE INTO github_write_outbox
            (intent_id,repository,target_kind,target_database_id,operation,content,
             request_digest,remote_database_id,lifecycle,next_attempt_at,created_at,updated_at)
         VALUES (?1,?2,?3,?4,'reaction_delete','rocket',?5,?6,'pending',?7,?7,?7)",
        params![
            Uuid::now_v7().to_string(),
            repository,
            target_kind,
            target_database_id,
            digest,
            remote_database_id,
            now,
        ],
    )?;
    Ok(())
}
