use sha2::Digest as _;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

use super::{
    StoreError, configure_connection, now_rfc3339, open_read_only, open_read_write,
    require_current_schema, sqlite_i64_to_u64, sqlite_u64,
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
    pub source: Option<super::publication_source::PublicationSource>,
}

#[derive(Debug, Clone)]
pub struct GhWriteReceipt {
    pub intent_id: String,
    pub request_id: String,
    pub execution_owner: Option<super::RuntimeLease>,
    pub recovery_until: Option<String>,
    pub source: Option<super::publication_source::PublicationSource>,
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
    pub root_intent_id: Option<String>,
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
    pub activation_delivered: bool,
    pub issue_node_id: String,
    pub pull_request_node_id: Option<String>,
    pub root_intent_id: String,
    pub root_request_id: String,
    pub root_target: String,
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
    if let Some(source) = &write.source {
        super::publication_source::validate(&transaction, source)?;
    }
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
               request_key,repository,target,profile_id,role,payload,source_binding
             ) VALUES (?1,?2,?3,'pending',0,?4,?4,?5,?6,?7,?8,?9,?10,?11)",
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
                write
                    .source
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| StoreError::InvalidData(e.to_string()))?,
            ],
        )?;
        if write.operation == "pr_handoff" {
            super::handoff::bind(&transaction, &intent_id, write)?;
        }
        intent_id
    };
    if write.operation == "pr_ensure" {
        let issue:Option<String>=transaction.query_row("SELECT i.issue_node_id FROM implementation_requests i JOIN implementation_requests root ON root.intent_id=COALESCE(i.root_intent_id,i.intent_id) WHERE i.intent_id=?1 AND i.pull_request_node_id IS NOT NULL AND COALESCE(i.activation_event_id,root.activation_event_id) IS NULL",[&intent_id],|r|r.get(0)).optional()?;
        if let Some(issue) = issue {
            if let Some(source) = &write.source {
                let caller: String = transaction.query_row(
                    "SELECT work_item_node_id FROM assignments WHERE assignment_id=?1",
                    [&source.assignment_id],
                    |r| r.get(0),
                )?;
                if caller != issue || source.role != "issue_agent" {
                    return Err(StoreError::InvalidData(
                        "implementation recovery must originate from its own Issue".into(),
                    ));
                }
            }
            transaction.execute("UPDATE write_intents SET lifecycle='pending',last_error=NULL,updated_at=?2 WHERE intent_id=?1 AND lifecycle IN ('applied','rejected','conflict')",params![intent_id,now_rfc3339()])?;
            transaction.execute("UPDATE github_write_outbox SET lifecycle='pending',next_attempt_at=?2,updated_at=?2,last_error=NULL WHERE intent_id=?1 AND lifecycle IN ('applied','rejected','conflict')",params![intent_id,now_rfc3339()])?;
        }
    }
    enqueue(&transaction, &intent_id)?;
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
               request_key,repository,target,profile_id,role,payload,source_binding
             ) VALUES (?1,'pr_ensure',?2,'pending',0,?3,?3,?4,?5,?6,?7,?8,?9,?10)",
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
                request
                    .write
                    .source
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()
                    .map_err(|e| StoreError::InvalidData(e.to_string()))?,
            ],
        )?;
        intent_id
    };
    let now = now_rfc3339();
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO implementation_requests(
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
    if inserted == 1
        && let Some(root) = &request.root_intent_id
    {
        let valid: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM implementation_requests WHERE intent_id=?1 AND root_intent_id IS NULL AND repository=?2 AND issue_node_id=?3 AND head_ref=?4 AND base_ref=?5 AND pr_profile_id=?6 AND pull_request_node_id IS NOT NULL AND pull_request_number IS NOT NULL)",params![root,request.write.repository,request.issue_node_id,request.head_ref,request.base_ref,request.pr_profile_id],|r|r.get(0))?;
        if !valid {
            return Err(StoreError::InvalidData(
                "continuation differs from the managed implementation identity".into(),
            ));
        }
        transaction.execute("UPDATE implementation_requests SET root_intent_id=?2,stage='activation_pending',bootstrap_authored_at=(SELECT bootstrap_authored_at FROM implementation_requests WHERE intent_id=?2),pull_request_database_id=(SELECT pull_request_database_id FROM implementation_requests WHERE intent_id=?2),pull_request_node_id=(SELECT pull_request_node_id FROM implementation_requests WHERE intent_id=?2),pull_request_number=(SELECT pull_request_number FROM implementation_requests WHERE intent_id=?2) WHERE intent_id=?1",params![intent_id,root])?;
    }
    enqueue(&transaction, &intent_id)?;
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
        || !matches!(write.operation, "comment_create" | "pr_ensure" | "body_edit" | "pr_handoff")
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

pub(super) fn gh_write_receipt_on(
    connection: &Connection,
    intent_id: &str,
) -> Result<Option<GhWriteReceipt>, StoreError> {
    connection
        .query_row(
            "SELECT w.intent_id,w.operation,w.repository,w.target,w.profile_id,w.role,
                    COALESCE(o.lifecycle,w.lifecycle),COALESCE(o.attempts,w.attempts),w.remote_database_id,
                    COALESCE(o.last_error,w.last_error),w.created_at,w.payload,w.public_request_id,w.source_binding
             FROM write_intents w LEFT JOIN github_write_outbox o ON o.intent_id=w.intent_id
             WHERE (w.intent_id=?1 OR w.public_request_id=?1) AND w.request_key IS NOT NULL",
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
                    request_id: row.get(12)?,
                    execution_owner: None,
                    recovery_until: None,
                    source: row.get::<_,Option<String>>(13)?.map(|value| serde_json::from_str(&value)).transpose().map_err(|e| rusqlite::Error::FromSqlConversionFailure(13,rusqlite::types::Type::Text,Box::new(e)))?,
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

pub(super) fn managed_implementations(
    database: &Path,
    repository: &str,
    head: &str,
) -> Result<Vec<ImplementationRequestReceipt>, StoreError> {
    require_current_schema(database)?;
    let connection = open_read_only(database)?;
    let mut statement=connection.prepare("SELECT intent_id FROM implementation_requests WHERE repository=?1 AND head_ref=?2 AND root_intent_id IS NULL ORDER BY updated_at DESC,intent_id")?;
    let ids = statement
        .query_map(params![repository, head], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.into_iter()
        .map(|id| {
            implementation_request_receipt_on(&connection, &id)?
                .ok_or_else(|| StoreError::InvalidData("managed implementation disappeared".into()))
        })
        .collect()
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
            "SELECT i.comment_database_id,i.issue_number,i.issue_title,
                    i.base_ref,i.head_ref,i.pr_profile_id,i.bootstrap_authored_at,
                    i.pull_request_number,i.stage,COALESCE(i.activation_event_id,root.activation_event_id) IS NOT NULL,
                    i.issue_node_id,i.pull_request_node_id,root.intent_id,rw.public_request_id,rw.target
             FROM implementation_requests i JOIN implementation_requests root ON root.intent_id=COALESCE(i.root_intent_id,i.intent_id)
             JOIN write_intents rw ON rw.intent_id=root.intent_id WHERE i.intent_id=?1",
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
                    activation_delivered: row.get(9)?,
                    issue_node_id: row.get(10)?,
                    pull_request_node_id: row.get(11)?,
                    root_intent_id: row.get(12)?,
                    root_request_id: row.get(13)?,
                    root_target: row.get(14)?,
                })
            },
        )
        .optional()
        .map_err(StoreError::from)
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

pub(super) fn require_owner(
    connection: &Connection,
    owner: &super::RuntimeLease,
) -> Result<(), StoreError> {
    let valid: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM owner_leases WHERE scope=?1 AND generation=?2 AND owner_id=?3 AND expires_at>?4)", params![owner.scope,sqlite_u64(owner.generation,"owner generation")?,owner.owner_id,now_rfc3339()], |row| row.get(0))?;
    if !valid || owner.scope != "runtime" {
        return Err(StoreError::InvalidData("outbox runtime owner expired or replaced".into()));
    }
    Ok(())
}

fn enqueue(connection: &Connection, intent_id: &str) -> Result<(), StoreError> {
    connection.execute("UPDATE write_intents SET public_request_id=COALESCE(public_request_id,?2) WHERE intent_id=?1", params![intent_id,format!("request-{}",Uuid::now_v7())])?;
    connection.execute("INSERT OR IGNORE INTO github_write_outbox(intent_id,repository,target_kind,target_database_id,operation,content,request_digest,lifecycle,next_attempt_at,created_at,updated_at)
        SELECT intent_id,repository,'agent',target,'agent_write',payload,?2,'pending',created_at,created_at,created_at FROM write_intents WHERE intent_id=?1", params![intent_id, hex::encode(sha2::Sha256::digest(format!("outbox:{intent_id}").as_bytes()))])?;
    Ok(())
}
