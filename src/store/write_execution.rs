use super::{
    OptionalExtension, Path, PendingGitHubWrite, RuntimeLease, StoreError, configure_connection,
    deadline_rfc3339, enqueue_rocket_removal, gh_writes, now_rfc3339, open_read_write, params,
    require_current_schema, settle_operational_status, sqlite_u64,
};

pub(super) fn recover_writes(database: &Path) -> Result<u64, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let changed = connection.execute(
        "UPDATE github_write_outbox SET lifecycle='uncertain',next_attempt_at=?1,updated_at=?1
         WHERE lifecycle='sending'",
        [now_rfc3339()],
    )?;
    Ok(u64::try_from(changed).expect("SQLite change count fits u64"))
}

pub(super) fn claim_github_write(
    database: &Path,
    owner: &RuntimeLease,
) -> Result<Option<PendingGitHubWrite>, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    gh_writes::require_owner(&transaction, owner)?;
    let recovery_until = deadline_rfc3339(5)?;
    let write = transaction
        .query_row(
            "SELECT intent_id,repository,target_kind,target_database_id,operation,content,
                    remote_database_id,lifecycle,created_at,attempts
             FROM github_write_outbox
             WHERE lifecycle IN ('pending','uncertain') AND next_attempt_at<=?1
             ORDER BY created_at,intent_id LIMIT 1",
            [&now],
            |row| {
                Ok(PendingGitHubWrite {
                    intent_id: row.get(0)?,
                    repository: row.get(1)?,
                    target_kind: row.get(2)?,
                    target_database_id: row.get(3)?,
                    operation: row.get(4)?,
                    content: row.get(5)?,
                    remote_database_id: row.get(6)?,
                    lifecycle: row.get(7)?,
                    created_at: row.get(8)?,
                    attempt: row.get::<_, i64>(9)? + 1,
                    owner: owner.clone(),
                    retry_after: 0,
                    recovery_until: recovery_until.clone(),
                })
            },
        )
        .optional()?;
    if let Some(write) = &write {
        transaction.execute(
            "UPDATE github_write_outbox SET lifecycle='sending',attempts=attempts+1,
               last_error=NULL,updated_at=?2,claim_owner=?3,claim_generation=?4 WHERE intent_id=?1",
            params![
                write.intent_id,
                now,
                owner.owner_id,
                sqlite_u64(owner.generation, "owner generation")?
            ],
        )?;
    }
    transaction.commit()?;
    Ok(write)
}

pub(super) fn finish_github_write(
    database: &Path,
    write: &PendingGitHubWrite,
    lifecycle: &str,
    remote_database_id: Option<&str>,
    remote_node_id: Option<&str>,
    error: Option<&str>,
) -> Result<(), StoreError> {
    let intent_id = &write.intent_id;
    require_current_schema(database)?;
    if !matches!(
        lifecycle,
        "applied" | "pending" | "uncertain" | "rejected" | "conflict" | "ambiguous"
    ) {
        return Err(StoreError::InvalidData(format!("invalid write terminal {lifecycle}")));
    }
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    gh_writes::require_owner(&transaction, &write.owner)?;
    let valid: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM github_write_outbox WHERE intent_id=?1 AND attempts=?2 AND claim_owner=?3 AND claim_generation=?4 AND lifecycle='sending')", params![intent_id, write.attempt, write.owner.owner_id, sqlite_u64(write.owner.generation, "owner generation")?], |row| row.get(0))?;
    if !valid {
        return Err(StoreError::InvalidData("stale outbox attempt".into()));
    }
    let (attempts, operation, repository, target_kind, target_database_id, content) = transaction
        .query_row(
            "SELECT attempts,operation,repository,target_kind,target_database_id,content
             FROM github_write_outbox WHERE intent_id=?1 AND lifecycle='sending'",
            [intent_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::InvalidData(format!("write intent {intent_id} is not sending"))
        })?;
    let retry_exponent = u32::try_from(attempts).unwrap_or(6).min(6);
    let retry_seconds = 2_u64.pow(retry_exponent).min(60);
    let next_attempt_at = if matches!(lifecycle, "uncertain" | "pending") {
        deadline_rfc3339(retry_seconds.max(10).max(write.retry_after))?
    } else {
        now_rfc3339()
    };
    // A fence may change after remote validation. Reject in this same short
    // transaction instead of leaving the executor's attempt stuck in sending.
    let delivery_error = if lifecycle == "applied" && operation == "agent_write" {
        super::handoff::settle(&transaction, write)?
    } else {
        None
    };
    let lifecycle = if delivery_error.is_some() { "rejected" } else { lifecycle };
    let error = delivery_error.as_deref().or(error);
    let updated = transaction.execute(
        "UPDATE github_write_outbox SET lifecycle=?2,remote_database_id=?3,last_error=?4,
           next_attempt_at=?5,updated_at=?6 WHERE intent_id=?1 AND lifecycle='sending'",
        params![intent_id, lifecycle, remote_database_id, error, next_attempt_at, now_rfc3339()],
    )?;
    if updated != 1 {
        return Err(StoreError::InvalidData(format!("write intent {intent_id} is not sending")));
    }
    transaction.execute("UPDATE write_intents SET lifecycle=?2,attempts=?3,remote_database_id=COALESCE(?4,remote_database_id),remote_node_id=COALESCE(?5,remote_node_id),last_error=?6,updated_at=?7 WHERE intent_id=?1", params![intent_id,lifecycle,attempts,remote_database_id,remote_node_id,error,now_rfc3339()])?;
    if lifecycle == "applied" && operation == "reaction_add" && content == "rocket" {
        let terminal_exists = transaction
            .query_row(
                "SELECT 1 FROM github_write_outbox
                 WHERE repository=?1 AND target_kind=?2 AND target_database_id=?3
                   AND operation='reaction_add' AND content IN ('+1','confused')
                   AND lifecycle NOT IN ('rejected','conflict','ambiguous','superseded') LIMIT 1",
                params![repository, target_kind, target_database_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if terminal_exists {
            enqueue_rocket_removal(
                &transaction,
                &repository,
                &target_kind,
                &target_database_id,
                &now_rfc3339(),
            )?;
        }
    }
    if matches!(operation.as_str(), "comment_create" | "comment_update") {
        settle_operational_status(
            &transaction,
            intent_id,
            lifecycle,
            remote_database_id,
            remote_node_id,
            &now_rfc3339(),
        )?;
    }
    transaction.commit()?;
    Ok(())
}
