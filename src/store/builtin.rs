//! Harness-owned upstream position and writer capabilities; never platform object state.
use super::{
    OptionalExtension, Path, StoreError, Uuid, configure_connection, open_read_write, params,
    require_current_schema, sqlite_i64_to_u64, sqlite_u64,
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
}

pub enum BuiltinCommand {
    State,
    Bind { space_id: String, repository: String, instance: String },
    Advance(u64),
    Grants,
    Grant(String),
    Prepare { session_id: String, item_number: u64, actor: String },
    Activate { session_id: String, grant_id: String, epoch: u64, token: String },
    Revoke { session_id: String, epoch: u64 },
}
pub enum BuiltinReply {
    Binding(BuiltinBinding),
    Grants(Vec<BuiltinGrant>),
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
    })
}
const SELECT: &str = "SELECT session_id,item_number,actor,request_id,epoch,grant_id,token,lifecycle FROM builtin_writer_grants";

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
        BuiltinCommand::State | BuiltinCommand::Grants | BuiltinCommand::Grant(_) => {
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
            tx.execute(
                "UPDATE builtin_upstream SET cursor=?1 WHERE singleton=1 AND cursor<=?1",
                [sqlite_u64(cursor, "upstream cursor")?],
            )?;
            BuiltinReply::Done
        }
        BuiltinCommand::Grants => {
            let rows = tx
                .prepare(&format!("{SELECT} WHERE lifecycle!='revoked'"))?
                .query_map([], grant)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            BuiltinReply::Grants(rows)
        }
        BuiltinCommand::Grant(session) => BuiltinReply::Grant(
            tx.query_row(&format!("{SELECT} WHERE session_id=?1"), [session], grant).optional()?,
        ),
        BuiltinCommand::Prepare { session_id, item_number, actor } => {
            let old = tx
                .query_row(&format!("{SELECT} WHERE session_id=?1"), [&session_id], grant)
                .optional()?;
            if old
                .as_ref()
                .is_some_and(|grant| grant.item_number != item_number || grant.actor != actor)
            {
                return Err(StoreError::InvalidData(
                    "provider session writer grant is bound to a different item/actor".into(),
                ));
            }
            if old.as_ref().is_some_and(|g| g.lifecycle != "revoked") {
                BuiltinReply::Grant(old)
            } else {
                let epoch:i64=tx.query_row("SELECT COALESCE(MAX(epoch),0) FROM builtin_writer_grants WHERE item_number=?1 AND actor=?2",params![sqlite_u64(item_number,"grant item")?,actor],|r|r.get(0))?;
                tx.execute("INSERT INTO builtin_writer_grants(session_id,item_number,actor,request_id,epoch,lifecycle) VALUES(?1,?2,?3,?4,?5,'pending') ON CONFLICT(session_id) DO UPDATE SET request_id=excluded.request_id,epoch=excluded.epoch,grant_id=NULL,token=NULL,lifecycle='pending'",params![session_id,sqlite_u64(item_number,"grant item")?,actor,Uuid::now_v7().to_string(),epoch])?;
                BuiltinReply::Grant(Some(tx.query_row(
                    &format!("{SELECT} WHERE session_id=?1"),
                    [session_id],
                    grant,
                )?))
            }
        }
        BuiltinCommand::Activate { session_id, grant_id, epoch, token } => {
            let updated=tx.execute("UPDATE builtin_writer_grants SET grant_id=?2,epoch=?3,token=?4,lifecycle='active' WHERE session_id=?1 AND lifecycle='pending'",params![session_id,grant_id,sqlite_u64(epoch,"grant epoch")?,token])?;
            if updated != 1 {
                return Err(StoreError::InvalidData(
                    "writer grant intent is no longer pending; refusing stale admission".into(),
                ));
            }
            BuiltinReply::Done
        }
        BuiltinCommand::Revoke { session_id, epoch } => {
            tx.execute("UPDATE builtin_writer_grants SET epoch=?2,token=NULL,lifecycle='revoked' WHERE session_id=?1",params![session_id,sqlite_u64(epoch,"grant epoch")?])?;
            BuiltinReply::Done
        }
    };
    tx.commit()?;
    Ok(reply)
}
