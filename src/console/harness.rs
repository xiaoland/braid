//! Read the Harness ledger without treating upstream objects or OTLP as execution authority.
use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result, ensure};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::config::Config;

#[derive(Clone)]
struct SnapshotState {
    database: PathBuf,
    repository: String,
    profiles: Value,
}

pub(super) fn router(config: &Config) -> Router {
    Router::new().route("/api/harness/snapshot", get(snapshot)).with_state(SnapshotState {
        database: config.runtime.database().into(),
        repository: config.repository().into(),
        profiles: serde_json::json!({
            "issue":config.profiles.iter().find(|p|p.tags.iter().any(|t|t=="issue")).map(|p|&p.id),
            "pr":config.profile_selection.default_pr_profile,
        }),
    })
}

async fn snapshot(
    State(state): State<SnapshotState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    tokio::task::spawn_blocking(move || read_snapshot(&state))
        .await
        .context("Harness snapshot task failed")
        .and_then(std::convert::identity)
        .map(Json)
        .map_err(|error| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error":format!("{error:#}")})),
            )
        })
}

fn read_snapshot(state: &SnapshotState) -> Result<Value> {
    let connection =
        Connection::open_with_flags(&state.database, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction = connection.unchecked_transaction()?;
    let version: u32 = transaction.query_row(
        "SELECT COALESCE(MAX(version),0) FROM schema_migrations",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        version == crate::store::DATABASE_SCHEMA_VERSION,
        "Harness database requires migration before reading its snapshot"
    );
    let result = serde_json::json!({
        "source":"braid_harness", "observedAt":OffsetDateTime::now_utc().format(&Rfc3339)?,
        "repository":state.repository,"profileSuggestions":state.profiles,
        "upstream":rows(&transaction,"SELECT space_id,repository,client_id,cursor FROM builtin_upstream WHERE singleton=1")?,
        "sessions":rows(&transaction,"SELECT session_id,agent_id,provider_kind,provider_session_id,lifecycle,started_at,resume_count,last_resumed_at,last_resume_error FROM provider_sessions ORDER BY started_at")?,
        "turns":rows(&transaction,"SELECT turn_id,session_id,provider_turn_id,trigger_kind,lifecycle,started_at,ended_at,error FROM turns ORDER BY started_at")?,
        "queue":rows(&transaction,"SELECT batch_id,work_item_node_id,event_count,quiet_deadline,urgent,lifecycle,created_at,updated_at FROM wake_batches ORDER BY created_at")?,
        "assignments":rows(&transaction,"SELECT assignment_id,work_item_node_id,generation,lifecycle,assigned_at,retired_at FROM assignments ORDER BY assigned_at")?,
        "worktrees":rows(&transaction,"SELECT worktree_id,agent_id,path,lifecycle,source_path,head_ref,local_branch FROM worktrees ORDER BY observed_at")?,
    });
    transaction.commit()?;
    Ok(result)
}

fn rows(connection: &Connection, sql: &str) -> Result<Vec<Value>> {
    let mut statement = connection.prepare(sql)?;
    let names = statement.column_names().into_iter().map(str::to_owned).collect::<Vec<_>>();
    Ok(statement
        .query_map([], |row| {
            let mut value = serde_json::Map::new();
            for (index, name) in names.iter().enumerate() {
                let field = match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Null => Value::Null,
                    rusqlite::types::ValueRef::Integer(n) => Value::from(n),
                    rusqlite::types::ValueRef::Real(n) => Value::from(n),
                    rusqlite::types::ValueRef::Text(text) => {
                        Value::String(String::from_utf8_lossy(text).into_owned())
                    }
                    rusqlite::types::ValueRef::Blob(_) => {
                        return Err(rusqlite::Error::InvalidColumnType(
                            index,
                            name.clone(),
                            rusqlite::types::Type::Blob,
                        ));
                    }
                };
                value.insert(name.clone(), field);
            }
            Ok(Value::Object(value))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}
