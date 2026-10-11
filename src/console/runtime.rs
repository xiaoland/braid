//! Owns actual runtime tasks; OTLP observations never drive these transitions.
use crate::{config::Config, health::HealthSnapshot};
use anyhow::{Context as _, Result, bail};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::{Mutex, RwLock, watch};

#[derive(Clone)]
pub struct Manager(Arc<Inner>);
struct Inner {
    config: Config,
    state: Mutex<Execution>,
    health: Arc<RwLock<HealthSnapshot>>,
}
struct Execution {
    generation: Option<String>,
    state: &'static str,
    error: Option<String>,
    provider_enabled: bool,
    stop: Option<watch::Sender<bool>>,
    monitor: Option<tokio::task::JoinHandle<()>>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Command {
    request_id: String,
    operation: Operation,
    expected_generation: Option<String>,
    #[serde(default)]
    transport_only: bool,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Start,
    Stop,
}
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error": self.1}))).into_response()
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

impl Manager {
    pub fn new(config: Config) -> Result<Self> {
        let mut initial = crate::runtime::initial_health(&config, false);
        initial.ready = false;
        let db = Connection::open_with_flags(
            config.runtime.database(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let unresolved:Option<String>=db.query_row("SELECT response_json FROM control_receipts WHERE scope='runtime' AND json_extract(response_json,'$.result.state') IN ('starting','running','stopping','blocked') ORDER BY created_at DESC LIMIT 1",[],|row|row.get(0)).optional()?;
        let generation = unresolved
            .as_ref()
            .map(|response| serde_json::from_str::<Value>(response))
            .transpose()?
            .and_then(|response| response["result"]["generation"].as_str().map(str::to_owned));
        let error=unresolved.map(|_|"previous Console ended without a persisted stop confirmation; verify its native executions before replacing this runtime".to_owned());
        let state = if error.is_some() { "blocked" } else { "stopped" };
        initial.last_error.clone_from(&error);
        let health = Arc::new(RwLock::new(initial));
        Ok(Self(Arc::new(Inner {
            config,
            health,
            state: Mutex::new(Execution {
                generation,
                state,
                error,
                provider_enabled: false,
                stop: None,
                monitor: None,
            }),
        })))
    }
    pub fn router(&self) -> Router {
        Router::new()
            .route("/api/runtime", get(status))
            .route("/api/runtime/commands", post(command))
            .route("/api/runtime/receipts/{request_id}", get(receipt))
            .with_state(self.clone())
    }
    pub async fn shutdown(&self) -> Result<()> {
        let monitor = {
            let mut state = self.0.state.lock().await;
            if let Some(stop) = &state.stop {
                let _ = stop.send(true);
                state.state = "stopping";
            }
            state.monitor.take()
        };
        if let Some(monitor) = monitor {
            tokio::time::timeout(std::time::Duration::from_secs(30), monitor)
                .await
                .context("runtime shutdown exceeded 30 seconds; execution stop is unconfirmed")?
                .context("runtime supervisor failed")?;
        }
        let state = self.0.state.lock().await;
        if state.state == "blocked" {
            bail!(
                "runtime stop is unconfirmed: {}",
                state.error.as_deref().unwrap_or("unknown failure")
            );
        }
        Ok(())
    }
}

async fn status(State(manager): State<Manager>) -> Json<Value> {
    let state = manager.0.state.lock().await;
    let health = manager.0.health.read().await.clone();
    let config = &manager.0.config;
    Json(
        json!({"instance":config.instance_key(),"repository":config.repository(),"generation":state.generation,"state":state.state,"error":state.error,"provider_enabled":state.provider_enabled,"health":health,"authority":"in-process runtime supervisor","evidence":crate::telemetry::evidence_health(),"telemetry":{"endpoint":config.telemetry.endpoint,"sample_ratio":config.telemetry.sample_ratio,"incident_mode":config.telemetry.incident_mode}}),
    )
}

async fn receipt(
    State(manager): State<Manager>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    read_receipt(manager.0.config.runtime.database().to_path_buf(), &id)
        .map_err(internal)?
        .map(Json)
        .ok_or_else(|| {
            ApiError(StatusCode::NOT_FOUND, format!("runtime receipt {id} does not exist"))
        })
}

async fn command(
    State(manager): State<Manager>,
    Json(command): Json<Command>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if command.request_id.trim().is_empty() || command.request_id.len() > 128 {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "request_id must contain 1 to 128 bytes".into(),
        ));
    }
    let payload = serde_json::to_string(&command).map_err(internal)?;
    let sha = hex::encode(Sha256::digest(payload.as_bytes()));
    let mut state = manager.0.state.lock().await;
    let db = Connection::open(manager.0.config.runtime.database()).map_err(internal)?;
    db.busy_timeout(std::time::Duration::from_secs(5)).map_err(internal)?;
    if let Some((saved_sha,response))=db.query_row("SELECT payload_sha256,response_json FROM control_receipts WHERE scope='runtime' AND request_id=?1",[&command.request_id],|row|Ok((row.get::<_,String>(0)?,row.get::<_,Option<String>>(1)?))).optional().map_err(internal)? {
        if saved_sha!=sha { return Err(ApiError(StatusCode::CONFLICT,"request_id already belongs to a different command".into())); }
        let response=response.map(|r|serde_json::from_str(&r)).transpose().map_err(internal)?.unwrap_or(json!({"request_id":command.request_id,"state":"prepared"}));
        return Ok((StatusCode::OK,Json(response)));
    }
    if command.expected_generation != state.generation {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("runtime generation changed; current generation is {:?}", state.generation),
        ));
    }
    match command.operation {
        Operation::Start if state.state != "stopped" => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                format!(
                    "cannot start runtime in state {}: {}",
                    state.state,
                    state.error.as_deref().unwrap_or("execution already owned")
                ),
            ));
        }
        Operation::Stop if !matches!(state.state, "starting" | "running" | "stopping") => {
            return Err(ApiError(
                StatusCode::CONFLICT,
                format!("cannot stop runtime in state {}", state.state),
            ));
        }
        _ => {}
    }
    let generation = match command.operation {
        Operation::Start => uuid::Uuid::now_v7().to_string(),
        Operation::Stop => {
            state.generation.clone().ok_or_else(|| internal("running runtime has no generation"))?
        }
    };
    let next_state = match command.operation {
        Operation::Start => "starting",
        Operation::Stop => "stopping",
    };
    let response = json!({"request_id":command.request_id,"state":"applied","result":{"accepted":true,"generation":generation,"state":next_state}});
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    db.execute("INSERT INTO control_receipts(scope,request_id,payload_sha256,request_json,state,response_json,created_at,updated_at) VALUES('runtime',?1,?2,?3,'applied',?4,?5,?5)",params![command.request_id,sha,payload,response.to_string(),now]).map_err(internal)?;
    drop(db);
    tracing::info!(name="braid.runtime.command", request.id=%command.request_id,
        braid.runtime.generation=%generation, operation=?command.operation,
        braid.outcome="accepted", "runtime command accepted by supervisor");
    match command.operation {
        Operation::Start => {
            start_runtime(&manager, &mut state, generation, !command.transport_only).await;
        }
        Operation::Stop => {
            state
                .stop
                .as_ref()
                .ok_or_else(|| internal("runtime stop handle is unavailable"))?
                .send(true)
                .map_err(internal)?;
            state.state = "stopping";
        }
    }
    Ok((StatusCode::ACCEPTED, Json(response)))
}
fn read_receipt(path: PathBuf, id: &str) -> Result<Option<Value>> {
    let db = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    db.query_row(
        "SELECT response_json FROM control_receipts WHERE scope='runtime' AND request_id=?1",
        [id],
        |row| row.get::<_, Option<String>>(0),
    )
    .optional()?
    .flatten()
    .map(|s| serde_json::from_str(&s).map_err(anyhow::Error::new))
    .transpose()
}
fn save_outcome(config: &Config, generation: &str, state: &str, error: Option<&str>) -> Result<()> {
    let db = Connection::open(config.runtime.database())?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    let mut query=db.prepare("SELECT request_id,response_json FROM control_receipts WHERE scope='runtime' AND json_extract(response_json,'$.result.generation')=?1")?;
    let rows = query
        .query_map([generation], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    for (id, response) in rows {
        let mut response: Value = serde_json::from_str(&response)?;
        response["result"]["state"] = json!(state);
        response["result"]["error"] = json!(error);
        db.execute("UPDATE control_receipts SET response_json=?1,error=?2,updated_at=?3 WHERE scope='runtime' AND request_id=?4",params![response.to_string(),error,time::OffsetDateTime::now_utc().unix_timestamp(),id])?;
    }
    Ok(())
}

async fn start_runtime(
    manager: &Manager,
    state: &mut Execution,
    generation: String,
    provider_enabled: bool,
) {
    let (stop, receiver) = watch::channel(false);
    state.generation = Some(generation.clone());
    state.state = "starting";
    state.error = None;
    state.provider_enabled = provider_enabled;
    state.stop = Some(stop);
    *manager.0.health.write().await =
        crate::runtime::initial_health(&manager.0.config, provider_enabled);
    crate::telemetry::set_runtime_generation(Some(generation.clone()));
    let task_manager = manager.clone();
    let task = tokio::spawn(crate::runtime::serve_managed(
        manager.0.config.clone(),
        provider_enabled,
        receiver,
        Arc::clone(&manager.0.health),
    ));
    state.state = "running";
    state.monitor = Some(tokio::spawn(async move {
        let result = task.await.map_err(anyhow::Error::new).and_then(std::convert::identity);
        let mut state = task_manager.0.state.lock().await;
        state.stop = None;
        match result {
            Ok(()) => {
                state.state = "stopped";
                state.error = None;
            }
            Err(error) => {
                state.state = "blocked";
                state.error = Some(format!("{error:#}"));
            }
        }
        tracing::info!(name="braid.runtime.terminal", braid.runtime.generation=%generation,
            braid.outcome=state.state, error.reason=state.error.as_deref().unwrap_or(""),
            "runtime supervision completed");
        crate::telemetry::set_runtime_generation(None);
        if let Err(error) =
            save_outcome(&task_manager.0.config, &generation, state.state, state.error.as_deref())
        {
            state.state = "blocked";
            state.error = Some(format!("runtime outcome persistence failed: {error:#}"));
        }
        let mut health = task_manager.0.health.write().await;
        health.ready = false;
        if state.error.is_some() {
            health.last_error.clone_from(&state.error);
        }
    }));
}
