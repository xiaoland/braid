//! The Harness owns native execution attempts and the evidence that they are fenced.
use super::{
    RuntimeLease, StoreError, Uuid, builtin::BuiltinReply, now_rfc3339, params, sqlite_u64,
};

pub enum BuiltinExecutionCommand {
    BindTurn { lease: RuntimeLease, turn_id: String },
    FenceSession { lease: RuntimeLease, provider_session: String },
    FenceAll(RuntimeLease),
    ConfirmFence { lease: RuntimeLease, execution_id: String },
}

impl BuiltinExecutionCommand {
    fn lease(&self) -> &RuntimeLease {
        match self {
            Self::BindTurn { lease, .. }
            | Self::FenceSession { lease, .. }
            | Self::FenceAll(lease)
            | Self::ConfirmFence { lease, .. } => lease,
        }
    }
}

pub(super) fn execute(
    tx: &rusqlite::Connection,
    command: BuiltinExecutionCommand,
) -> Result<BuiltinReply, StoreError> {
    let lease = command.lease();
    let owned: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM owner_leases WHERE scope='runtime' AND owner_id=?1 AND
         generation=?2 AND expires_at>?3)",
        params![
            lease.owner_id,
            sqlite_u64(lease.generation, "owner lease generation")?,
            now_rfc3339()
        ],
        |row| row.get(0),
    )?;
    if lease.scope != "runtime" || !owned {
        return Err(StoreError::InvalidData("runtime execution lease is no longer current".into()));
    }
    match command {
        BuiltinExecutionCommand::BindTurn { lease, turn_id } => bind_turn(tx, &lease, &turn_id),
        BuiltinExecutionCommand::FenceSession { provider_session, .. } => {
            let ids = tx
                .prepare(
                    "UPDATE provider_sessions
                    SET builtin_execution_state=CASE WHEN builtin_execution_id IS NULL
                        THEN 'fenced' ELSE 'fencing' END
                    WHERE provider_session_id=?1 AND builtin_execution_state!='fenced'
                    RETURNING builtin_execution_id",
                )?
                .query_map([provider_session], |row| row.get::<_, Option<String>>(0))?
                .filter_map(Result::transpose)
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(BuiltinReply::Fences(ids))
        }
        BuiltinExecutionCommand::FenceAll(_) => {
            let ids = tx
                .prepare(
                    "UPDATE provider_sessions SET builtin_execution_state='fencing'
                    WHERE builtin_execution_id IS NOT NULL AND builtin_execution_state!='fenced'
                    RETURNING builtin_execution_id",
                )?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(BuiltinReply::Fences(ids))
        }
        BuiltinExecutionCommand::ConfirmFence { execution_id, .. } => {
            tx.execute(
                "UPDATE provider_sessions SET builtin_execution_state='fenced'
                 WHERE builtin_execution_id=?1 AND builtin_execution_state='fencing'",
                [execution_id],
            )?;
            Ok(BuiltinReply::Done)
        }
    }
}

fn bind_turn(
    tx: &rusqlite::Connection,
    lease: &RuntimeLease,
    turn_id: &str,
) -> Result<BuiltinReply, StoreError> {
    let execution = tx.query_row(
        "UPDATE provider_sessions SET builtin_execution_id=COALESCE(builtin_execution_id,?2),
               builtin_execution_owner=?3
         WHERE session_id=(SELECT session_id FROM turns WHERE turn_id=?1 AND lifecycle='starting')
           AND lifecycle='running' AND builtin_execution_state='open'
           AND (builtin_execution_owner IS NULL OR builtin_execution_owner=?3)
         RETURNING builtin_execution_id",
        params![turn_id, Uuid::now_v7().to_string(), lease.owner_id],
        |row| row.get::<_, String>(0),
    );
    let execution = match execution {
        Ok(id) => id,
        Err(rusqlite::Error::QueryReturnedNoRows) => {
            return Err(StoreError::InvalidData(
                "turn cannot bind an open native execution".into(),
            ));
        }
        Err(error) => return Err(error.into()),
    };
    tx.execute(
        "UPDATE turns SET builtin_execution_id=?2 WHERE turn_id=?1",
        params![turn_id, execution],
    )?;
    Ok(BuiltinReply::Done)
}
