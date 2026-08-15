use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::{
    StoreError, configure_connection, deadline_rfc3339, now_rfc3339, open_read_only,
    open_read_write, require_current_schema, sqlite_i64_to_u64, sqlite_u64,
};

#[derive(Debug, Clone)]
pub struct NewGhWriteIntent {
    pub request_key: String,
    pub operation: &'static str,
    pub repository: String,
    pub target: String,
    pub profile_id: String,
    pub role: String,
    pub payload: String,
    pub request_digest: String,
}

#[derive(Debug, Clone)]
pub struct GhWriteReceipt {
    pub intent_id: String,
    pub operation: String,
    pub repository: String,
    pub target: String,
    pub profile_id: String,
    pub role: String,
    pub lifecycle: String,
    pub attempts: u64,
    pub remote_database_id: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub payload: String,
}

#[derive(Debug, Clone)]
pub struct NewImplementationRequest {
    pub write: NewGhWriteIntent,
    pub comment_database_id: u64,
    pub comment_node_id: String,
    pub issue_node_id: String,
    pub issue_number: u64,
    pub issue_title: String,
    pub base_ref: String,
    pub head_ref: String,
    pub pr_profile_id: String,
}

#[derive(Debug, Clone)]
pub struct ImplementationRequestReceipt {
    pub write: GhWriteReceipt,
    pub comment_database_id: u64,
    pub issue_number: u64,
    pub issue_title: String,
    pub base_ref: String,
    pub head_ref: String,
    pub pr_profile_id: String,
    pub bootstrap_authored_at: String,
    pub pull_request_number: Option<u64>,
    pub stage: String,
}

#[derive(Debug, Clone)]
pub struct ImplementationProgress {
    pub stage: &'static str,
    pub bootstrap_commit_sha: Option<String>,
    pub pull_request_database_id: Option<u64>,
    pub pull_request_node_id: Option<String>,
    pub pull_request_number: Option<u64>,
}

pub(super) fn prepare_gh_write(
    database: &Path,
    write: &NewGhWriteIntent,
) -> Result<GhWriteReceipt, StoreError> {
    require_current_schema(database)?;
    validate_gh_write(write)?;
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let existing = transaction
        .query_row(
            "SELECT intent_id,request_digest FROM write_intents WHERE request_key=?1",
            [&write.request_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let intent_id = if let Some((intent_id, digest)) = existing {
        if digest != write.request_digest {
            return Err(StoreError::InvalidData(format!(
                "write request key {:?} was already used for a different payload",
                write.request_key
            )));
        }
        intent_id
    } else {
        let intent_id = Uuid::now_v7().to_string();
        let now = now_rfc3339();
        transaction.execute(
            "INSERT INTO write_intents(
               intent_id,operation,request_digest,lifecycle,attempts,created_at,updated_at,
               request_key,repository,target,profile_id,role,payload
             ) VALUES (?1,?2,?3,'pending',0,?4,?4,?5,?6,?7,?8,?9,?10)",
            params![
                intent_id,
                write.operation,
                write.request_digest,
                now,
                write.request_key,
                write.repository,
                write.target,
                write.profile_id,
                write.role,
                write.payload,
            ],
        )?;
        intent_id
    };
    let receipt = gh_write_receipt_on(&transaction, &intent_id)?.ok_or_else(|| {
        StoreError::InvalidData(format!("write intent {intent_id} disappeared during prepare"))
    })?;
    transaction.commit()?;
    Ok(receipt)
}

pub(super) fn prepare_implementation_request(
    database: &Path,
    request: &NewImplementationRequest,
) -> Result<ImplementationRequestReceipt, StoreError> {
    require_current_schema(database)?;
    validate_gh_write(&request.write)?;
    if request.write.operation != "pr_ensure" || request.comment_database_id == 0 {
        return Err(StoreError::InvalidData("invalid implementation request intent".into()));
    }
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let existing = transaction
        .query_row(
            "SELECT intent_id,request_digest FROM write_intents WHERE request_key=?1",
            [&request.write.request_key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let intent_id = if let Some((intent_id, digest)) = existing {
        if digest != request.write.request_digest {
            return Err(StoreError::InvalidData(format!(
                "Implementation Request {} was already planned with different arguments",
                request.comment_database_id
            )));
        }
        intent_id
    } else {
        let intent_id = Uuid::now_v7().to_string();
        let now = now_rfc3339();
        transaction.execute(
            "INSERT INTO write_intents(
               intent_id,operation,request_digest,lifecycle,attempts,created_at,updated_at,
               request_key,repository,target,profile_id,role,payload
             ) VALUES (?1,'pr_ensure',?2,'pending',0,?3,?3,?4,?5,?6,?7,?8,?9)",
            params![
                intent_id,
                request.write.request_digest,
                now,
                request.write.request_key,
                request.write.repository,
                request.write.target,
                request.write.profile_id,
                request.write.role,
                request.write.payload,
            ],
        )?;
        transaction.execute(
            "INSERT INTO implementation_requests(
               intent_id,repository,comment_database_id,comment_node_id,issue_node_id,issue_number,
               issue_title,base_ref,head_ref,pr_profile_id,bootstrap_authored_at,stage,updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'planned',?11)",
            params![
                intent_id,
                request.write.repository,
                sqlite_u64(request.comment_database_id, "Implementation Request comment ID",)?,
                request.comment_node_id,
                request.issue_node_id,
                sqlite_u64(request.issue_number, "Implementation Request Issue number")?,
                request.issue_title,
                request.base_ref,
                request.head_ref,
                request.pr_profile_id,
                now,
            ],
        )?;
        intent_id
    };
    let receipt =
        implementation_request_receipt_on(&transaction, &intent_id)?.ok_or_else(|| {
            StoreError::InvalidData(format!(
                "implementation request {intent_id} disappeared during prepare"
            ))
        })?;
    transaction.commit()?;
    Ok(receipt)
}

fn validate_gh_write(write: &NewGhWriteIntent) -> Result<(), StoreError> {
    if write.request_key.is_empty()
        || write.repository.is_empty()
        || write.target.is_empty()
        || write.profile_id.is_empty()
        || write.role.is_empty()
        || write.payload.is_empty()
        || write.request_digest.len() != 64
        || !matches!(write.operation, "comment_create" | "pr_ensure")
    {
        return Err(StoreError::InvalidData("invalid braid gh write intent".into()));
    }
    Ok(())
}

pub(super) fn gh_write_receipt(
    database: &Path,
    intent_id: &str,
) -> Result<Option<GhWriteReceipt>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    gh_write_receipt_on(&connection, intent_id)
}

fn gh_write_receipt_on(
    connection: &Connection,
    intent_id: &str,
) -> Result<Option<GhWriteReceipt>, StoreError> {
    connection
        .query_row(
            "SELECT intent_id,operation,repository,target,profile_id,role,
                    lifecycle,attempts,remote_database_id,
                    last_error,created_at,payload
             FROM write_intents WHERE intent_id=?1 AND request_key IS NOT NULL",
            [intent_id],
            |row| {
                Ok(GhWriteReceipt {
                    intent_id: row.get(0)?,
                    operation: row.get(1)?,
                    repository: row.get(2)?,
                    target: row.get(3)?,
                    profile_id: row.get(4)?,
                    role: row.get(5)?,
                    lifecycle: row.get(6)?,
                    attempts: sqlite_i64_to_u64(row.get(7)?, "write attempts")?,
                    remote_database_id: row.get(8)?,
                    last_error: row.get(9)?,
                    created_at: row.get(10)?,
                    payload: row.get(11)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

pub(super) fn implementation_request_receipt(
    database: &Path,
    intent_id: &str,
) -> Result<Option<ImplementationRequestReceipt>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    implementation_request_receipt_on(&connection, intent_id)
}

fn implementation_request_receipt_on(
    connection: &Connection,
    intent_id: &str,
) -> Result<Option<ImplementationRequestReceipt>, StoreError> {
    let Some(write) = gh_write_receipt_on(connection, intent_id)? else {
        return Ok(None);
    };
    connection
        .query_row(
            "SELECT comment_database_id,issue_number,issue_title,
                    base_ref,head_ref,pr_profile_id,bootstrap_authored_at,
                    pull_request_number,stage
             FROM implementation_requests WHERE intent_id=?1",
            [intent_id],
            |row| {
                Ok(ImplementationRequestReceipt {
                    write: write.clone(),
                    comment_database_id: sqlite_i64_to_u64(
                        row.get(0)?,
                        "Implementation Request comment ID",
                    )?,
                    issue_number: sqlite_i64_to_u64(row.get(1)?, "Issue number")?,
                    issue_title: row.get(2)?,
                    base_ref: row.get(3)?,
                    head_ref: row.get(4)?,
                    pr_profile_id: row.get(5)?,
                    bootstrap_authored_at: row.get(6)?,
                    pull_request_number: row
                        .get::<_, Option<i64>>(7)?
                        .map(|value| sqlite_i64_to_u64(value, "pull request number"))
                        .transpose()?,
                    stage: row.get(8)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
}

pub(super) fn claim_gh_write(database: &Path, intent_id: &str) -> Result<bool, StoreError> {
    require_current_schema(database)?;
    let now = now_rfc3339();
    let expires_at = deadline_rfc3339(60)?;
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let changed = connection.execute(
        "UPDATE write_intents SET lifecycle='sending',attempts=attempts+1,
           claim_expires_at=?2,last_error=NULL,updated_at=?1
         WHERE intent_id=?3 AND request_key IS NOT NULL
           AND (lifecycle IN ('pending','uncertain')
                OR (lifecycle='sending' AND claim_expires_at<=?1))",
        params![now, expires_at, intent_id],
    )?;
    Ok(changed == 1)
}

pub(super) fn record_implementation_progress(
    database: &Path,
    intent_id: &str,
    progress: &ImplementationProgress,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    let stage_rank = implementation_stage_rank(progress.stage)?;
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let transaction = connection.transaction()?;
    let (current_stage, current_bootstrap, current_pr_id, current_pr_node, current_pr_number) =
        transaction.query_row(
            "SELECT stage,bootstrap_commit_sha,pull_request_database_id,pull_request_node_id,
                    pull_request_number FROM implementation_requests WHERE intent_id=?1",
            [intent_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                ))
            },
        )?;
    if stage_rank < implementation_stage_rank(&current_stage)? {
        return Err(StoreError::InvalidData(format!(
            "implementation request {intent_id} cannot move backward from {current_stage} to {}",
            progress.stage
        )));
    }
    reject_progress_change(
        "bootstrap commit",
        current_bootstrap.as_deref(),
        progress.bootstrap_commit_sha.as_deref(),
    )?;
    reject_progress_change(
        "pull request database ID",
        current_pr_id.as_ref().map(ToString::to_string).as_deref(),
        progress.pull_request_database_id.map(|value| value.to_string()).as_deref(),
    )?;
    reject_progress_change(
        "pull request node ID",
        current_pr_node.as_deref(),
        progress.pull_request_node_id.as_deref(),
    )?;
    reject_progress_change(
        "pull request number",
        current_pr_number.as_ref().map(ToString::to_string).as_deref(),
        progress.pull_request_number.map(|value| value.to_string()).as_deref(),
    )?;
    transaction.execute(
        "UPDATE implementation_requests SET stage=?2,
           bootstrap_commit_sha=COALESCE(bootstrap_commit_sha,?3),
           pull_request_database_id=COALESCE(pull_request_database_id,?4),
           pull_request_node_id=COALESCE(pull_request_node_id,?5),
           pull_request_number=COALESCE(pull_request_number,?6),updated_at=?7
         WHERE intent_id=?1",
        params![
            intent_id,
            progress.stage,
            progress.bootstrap_commit_sha,
            progress
                .pull_request_database_id
                .map(|value| sqlite_u64(value, "pull request database ID"))
                .transpose()?,
            progress.pull_request_node_id,
            progress
                .pull_request_number
                .map(|value| sqlite_u64(value, "pull request number"))
                .transpose()?,
            now_rfc3339(),
        ],
    )?;
    transaction.commit()?;
    Ok(())
}

fn implementation_stage_rank(stage: &str) -> Result<u8, StoreError> {
    match stage {
        "planned" => Ok(0),
        "head_ready" => Ok(1),
        "pull_request_ready" => Ok(2),
        "associated" => Ok(3),
        "activation_pending" => Ok(4),
        _ => Err(StoreError::InvalidData(format!("invalid implementation stage {stage:?}"))),
    }
}

fn reject_progress_change(
    field: &str,
    current: Option<&str>,
    proposed: Option<&str>,
) -> Result<(), StoreError> {
    if current.is_some() && proposed.is_some() && current != proposed {
        return Err(StoreError::InvalidData(format!(
            "implementation {field} changed from {current:?} to {proposed:?}"
        )));
    }
    Ok(())
}

pub(super) fn finish_gh_write(
    database: &Path,
    intent_id: &str,
    lifecycle: &str,
    remote_database_id: Option<&str>,
    remote_node_id: Option<&str>,
    remote_url: Option<&str>,
    error: Option<&str>,
) -> Result<(), StoreError> {
    require_current_schema(database)?;
    if !matches!(lifecycle, "applied" | "uncertain" | "rejected" | "conflict" | "ambiguous") {
        return Err(StoreError::InvalidData(format!("invalid braid gh terminal {lifecycle}")));
    }
    let connection = open_read_write(database)?;
    configure_connection(&connection)?;
    let changed = connection.execute(
        "UPDATE write_intents SET lifecycle=?2,remote_database_id=COALESCE(?3,remote_database_id),
           remote_node_id=COALESCE(?4,remote_node_id),remote_url=COALESCE(?5,remote_url),
           last_error=?6,claim_expires_at=NULL,updated_at=?7
         WHERE intent_id=?1 AND request_key IS NOT NULL AND lifecycle='sending'",
        params![
            intent_id,
            lifecycle,
            remote_database_id,
            remote_node_id,
            remote_url,
            error,
            now_rfc3339(),
        ],
    )?;
    if changed != 1 {
        return Err(StoreError::InvalidData(format!(
            "braid gh write intent {intent_id} is not sending"
        )));
    }
    Ok(())
}
