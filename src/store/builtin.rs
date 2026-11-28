//! Harness-owned upstream position and writer capabilities; never platform object state.
use super::{
    BuiltinExecutionCommand, OptionalExtension, Path, StoreError, Uuid, builtin_execution,
    configure_connection, open_read_write, params, require_current_schema, sqlite_i64_to_u64,
    sqlite_u64,
};

#[derive(Debug, Clone)]
pub struct BuiltinBinding {
    pub space_id: String,
    pub repository: String,
    pub client_id: String,
    pub cursor: u64,
}
#[derive(Debug, Clone)]
pub struct BuiltinGrant {
    pub session_id: String,
    pub item_number: u64,
    pub actor: String,
    pub request_id: String,
    pub epoch: u64,
    pub grant_id: Option<String>,
    pub token: Option<String>,
    pub lifecycle: String,
    pub authority_cursor: Option<u64>,
    pub execution_id: String,
}

#[derive(Clone)]
pub struct BuiltinWriterTurn {
    pub session_id: String,
    pub turn_id: String,
    pub item_number: u64,
    pub actor: String,
}

pub enum BuiltinCommand {
    State,
    Bind { space_id: String, repository: String, instance: String },
    Advance(u64),
    Execution(BuiltinExecutionCommand),
    WriterGrant(BuiltinWriterTurn),
    BeginMint { turn: BuiltinWriterTurn, expected_epoch: u64 },
    Activate { request_id: String, session_id: String, grant_id: String, epoch: u64, token: String },
    RejectMint { request_id: String, session_id: String },
}
pub enum BuiltinReply {
    Binding(BuiltinBinding),
    Fences(Vec<String>),
    Grant(Option<BuiltinGrant>),
    Done,
}

fn grant(row: &rusqlite::Row<'_>) -> rusqlite::Result<BuiltinGrant> {
    Ok(BuiltinGrant {
        session_id: row.get(0)?,
        item_number: sqlite_i64_to_u64(row.get(1)?, "grant item")?,
        actor: row.get(2)?,
        request_id: row.get(3)?,
        epoch: sqlite_i64_to_u64(row.get(4)?, "grant epoch")?,
        grant_id: row.get(5)?,
        token: row.get(6)?,
        lifecycle: row.get(7)?,
        authority_cursor: row
            .get::<_, Option<i64>>(8)?
            .map(|cursor| sqlite_i64_to_u64(cursor, "grant authority cursor"))
            .transpose()?,
        execution_id: row.get(9)?,
    })
}
const SELECT: &str = "SELECT session_id,item_number,actor,request_id,epoch,grant_id,token,lifecycle,authority_cursor,execution_id FROM builtin_writer_grants";

fn session_grant(
    tx: &rusqlite::Connection,
    session_id: &str,
    execution_id: &str,
) -> Result<Option<BuiltinGrant>, StoreError> {
    Ok(tx
        .query_row(
            &format!("{SELECT} WHERE session_id=?1 AND execution_id=?2"),
            params![session_id, execution_id],
            grant,
        )
        .optional()?)
}

pub(super) fn cursor_at_turn_claim(
    tx: &rusqlite::Connection,
    batch: &str,
    trigger: &str,
) -> Result<Option<i64>, StoreError> {
    // An unprocessed ownership transition may already be in this cursor.
    // The running turn can compute, but only a settled generation may mint.
    Ok(tx
        .query_row(
            "SELECT cursor FROM builtin_upstream WHERE singleton=1 AND ?2!='finalization'
         AND NOT EXISTS (
           SELECT 1 FROM events e JOIN wake_batches b ON b.work_item_node_id=e.work_item_node_id
           WHERE b.batch_id=?1 AND e.lifecycle IN ('pending','materializing')
             AND (e.kind IN ('assign','unassign') OR
                  (e.kind='lifecycle' AND e.detail IN ('closed','merged')))
         )",
            params![batch, trigger],
            |row| row.get(0),
        )
        .optional()?)
}

fn writer_execution(
    tx: &rusqlite::Connection,
    turn: &BuiltinWriterTurn,
) -> Result<(i64, String), StoreError> {
    let cursor = tx
        .query_row(
            "SELECT t.builtin_writer_cursor,t.builtin_execution_id
         FROM turns t JOIN provider_sessions s ON s.session_id=t.session_id
         JOIN agent_instances g ON g.agent_id=s.agent_id
         JOIN assignments a ON a.assignment_id=g.assignment_id
         JOIN work_items i ON i.node_id=a.work_item_node_id
         WHERE t.turn_id=?1 AND s.provider_session_id=?2 AND i.number=?3 AND g.profile_id=?4
           AND t.lifecycle IN ('starting','running') AND s.lifecycle='running'
           AND a.lifecycle='active' AND g.lifecycle='idle' AND t.builtin_writer_cursor IS NOT NULL
           AND s.builtin_execution_id=t.builtin_execution_id AND s.builtin_execution_state='open'
           AND EXISTS (SELECT 1 FROM owner_leases l WHERE l.scope='runtime'
                       AND l.owner_id=s.builtin_execution_owner AND l.expires_at>?5)",
            params![
                turn.turn_id,
                turn.session_id,
                sqlite_u64(turn.item_number, "grant item")?,
                turn.actor,
                super::now_rfc3339()
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    cursor
        .ok_or_else(|| StoreError::InvalidData("Agent turn has no current writer authority".into()))
}

fn writer_grant(
    tx: &rusqlite::Connection,
    turn: &BuiltinWriterTurn,
    execution_id: &str,
) -> Result<Option<BuiltinGrant>, StoreError> {
    let current = session_grant(tx, &turn.session_id, execution_id)?;
    if current
        .as_ref()
        .is_some_and(|grant| grant.item_number != turn.item_number || grant.actor != turn.actor)
    {
        return Err(StoreError::InvalidData(
            "writer capability is fenced or belongs to another responsibility".into(),
        ));
    }
    Ok(current)
}

fn begin_mint(
    tx: &rusqlite::Connection,
    turn: &BuiltinWriterTurn,
    expected_epoch: u64,
) -> Result<BuiltinReply, StoreError> {
    let (cursor, execution_id) = writer_execution(tx, turn)?;
    if let Some(current) = writer_grant(tx, turn, &execution_id)?
        && (current.lifecycle == "pending"
            || (current.lifecycle == "active" && current.epoch > expected_epoch))
    {
        return Ok(BuiltinReply::Grant(Some(current)));
    }
    tx.execute(
        "INSERT INTO builtin_writer_grants(session_id,item_number,actor,request_id,epoch,lifecycle,authority_cursor,execution_id)
         VALUES(?1,?2,?3,?4,?5,'pending',?6,?7)
         ON CONFLICT(session_id) DO UPDATE SET request_id=excluded.request_id,epoch=excluded.epoch,
             authority_cursor=excluded.authority_cursor,execution_id=excluded.execution_id,
             grant_id=NULL,token=NULL,lifecycle='pending'",
        params![turn.session_id,sqlite_u64(turn.item_number,"grant item")?,turn.actor,
            Uuid::now_v7().to_string(),sqlite_u64(expected_epoch,"grant epoch")?,cursor,execution_id],
    )?;
    Ok(BuiltinReply::Grant(session_grant(tx, &turn.session_id, &execution_id)?))
}

pub(super) fn advance_cursor(tx: &rusqlite::Connection, cursor: u64) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE builtin_upstream SET cursor=?1 WHERE singleton=1 AND cursor<=?1",
        [sqlite_u64(cursor, "upstream cursor")?],
    )?;
    Ok(())
}

// Every command uses the same transaction boundary and commits exactly once.
#[allow(clippy::too_many_lines)]
pub(super) fn execute(
    database: &Path,
    command: BuiltinCommand,
) -> Result<BuiltinReply, StoreError> {
    require_current_schema(database)?;
    let mut connection = open_read_write(database)?;
    configure_connection(&connection)?;
    // CLI callers share this ledger with the runtime; reserve writes before reading their snapshot.
    let behavior = match &command {
        BuiltinCommand::State | BuiltinCommand::WriterGrant(_) => {
            rusqlite::TransactionBehavior::Deferred
        }
        _ => rusqlite::TransactionBehavior::Immediate,
    };
    let tx = connection.transaction_with_behavior(behavior)?;
    let reply = match command {
        BuiltinCommand::State => BuiltinReply::Binding(tx.query_row(
            "SELECT space_id,repository,client_id,cursor FROM builtin_upstream WHERE singleton=1",
            [],
            |row| {
                Ok(BuiltinBinding {
                    space_id: row.get(0)?,
                    repository: row.get(1)?,
                    client_id: row.get(2)?,
                    cursor: sqlite_i64_to_u64(row.get(3)?, "upstream cursor")?,
                })
            },
        )?),
        BuiltinCommand::Bind { space_id, repository, instance } => {
            let mismatched: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM repositories WHERE node_id!=?1 OR name_with_owner!=?2)",
                params![format!("local:{repository}"), repository],
                |row| row.get(0),
            )?;
            let legacy: Option<String> = tx
                .query_row("SELECT repository FROM local_space WHERE singleton=1", [], |row| {
                    row.get(0)
                })
                .optional()?;
            if mismatched || legacy.as_ref().is_some_and(|name| name != &repository) {
                return Err(StoreError::InvalidData(
                    "Harness ledger contains a different upstream repository; use its original authority or a separate ledger".into(),
                ));
            }
            tx.execute("INSERT OR IGNORE INTO builtin_upstream(singleton,space_id,repository,client_id) VALUES(1,?1,?2,?3)",params![space_id,repository,format!("{instance}:{}",Uuid::now_v7())])?;
            let binding=tx.query_row("SELECT space_id,repository,client_id,cursor FROM builtin_upstream WHERE singleton=1",[],|row|Ok(BuiltinBinding{space_id:row.get(0)?,repository:row.get(1)?,client_id:row.get(2)?,cursor:sqlite_i64_to_u64(row.get(3)?,"upstream cursor")?}))?;
            if binding.space_id != space_id || binding.repository != repository {
                return Err(StoreError::InvalidData("Harness ledger is bound to a different built-in platform space; refusing authority switch".into()));
            }
            BuiltinReply::Binding(binding)
        }
        BuiltinCommand::Advance(cursor) => {
            advance_cursor(&tx, cursor)?;
            BuiltinReply::Done
        }
        BuiltinCommand::Execution(command) => builtin_execution::execute(&tx, command)?,
        BuiltinCommand::WriterGrant(turn) => {
            let (_, execution_id) = writer_execution(&tx, &turn)?;
            BuiltinReply::Grant(writer_grant(&tx, &turn, &execution_id)?)
        }
        BuiltinCommand::BeginMint { turn, expected_epoch } => {
            begin_mint(&tx, &turn, expected_epoch)?
        }
        BuiltinCommand::Activate { request_id, session_id, grant_id, epoch, token } => {
            let updated = tx.execute(
                "UPDATE builtin_writer_grants SET grant_id=?3,epoch=?4,token=?5,lifecycle='active'
                 WHERE session_id=?1 AND request_id=?2
                   AND EXISTS(SELECT 1 FROM provider_sessions s WHERE
                     s.builtin_execution_id=builtin_writer_grants.execution_id AND
                     s.builtin_execution_state='open')
                   AND (lifecycle='pending' OR (lifecycle='active' AND grant_id=?3 AND epoch=?4 AND token=?5))",
                params![session_id,request_id,grant_id,sqlite_u64(epoch,"grant epoch")?,token],
            )?;
            if updated != 1 {
                return Err(StoreError::InvalidData(
                    "writer grant intent is no longer current".into(),
                ));
            }
            BuiltinReply::Done
        }
        BuiltinCommand::RejectMint { request_id, session_id } => {
            tx.execute(
                "UPDATE builtin_writer_grants SET lifecycle='revoked'
                 WHERE session_id=?1 AND request_id=?2 AND lifecycle='pending'",
                params![session_id, request_id],
            )?;
            BuiltinReply::Done
        }
    };
    tx.commit()?;
    Ok(reply)
}
