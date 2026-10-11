//! HTTP access to Braid's authoritative local objects; OTLP is observation only.
use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use axum::{
    Json, Router,
    extract::{Path, State, rejection::JsonRejection},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::Value;

use crate::{
    config::Config,
    local::objects::{CommandFailure, CommandRequest, ControlPolicy, ControlReceipt, LocalObjects},
};

struct ControlState {
    database: std::path::PathBuf,
    repository: String,
    policy: ControlPolicy,
    commands: Mutex<()>,
}

pub fn router(config: &Config) -> Result<Router> {
    let local =
        config.local.as_ref().context("HTTP object control requires local collaboration")?;
    let issue_profile = config
        .profiles
        .iter()
        .find(|p| p.tags.iter().any(|t| t == "issue"))
        .context("no Issue profile configured")?
        .id
        .clone();
    let state = Arc::new(ControlState {
        database: config.runtime.database().into(),
        repository: local.repository.clone(),
        commands: Mutex::new(()),
        policy: ControlPolicy {
            source: local.source.clone(),
            issue_profile,
            pr_profile: config.profile_selection.default_pr_profile.clone(),
        },
    });
    let objects = LocalObjects::open(&state.database, &state.repository)?;
    objects.bind_source(&state.policy.source)?;
    Ok(Router::new()
        .route("/api/control/snapshot", get(snapshot))
        .route("/api/control/items/{number}", get(item))
        .route("/api/control/items/{number}/diff", get(diff))
        .route("/api/control/receipts/{request_id}", get(receipt))
        .route("/api/control/commands", post(command))
        .with_state(state))
}

async fn snapshot(State(state): State<Arc<ControlState>>) -> Result<Json<Value>, HttpError> {
    let profiles =
        serde_json::json!({"issue":state.policy.issue_profile,"pr":state.policy.pr_profile});
    let mut snapshot = run_read(state, LocalObjects::control_snapshot).await?;
    snapshot["supportedProfiles"] = profiles;
    Ok(Json(snapshot))
}
async fn item(
    State(state): State<Arc<ControlState>>,
    Path(number): Path<u64>,
) -> Result<Json<Value>, HttpError> {
    run_read(state, move |objects| objects.control_item(number)).await.map(Json)
}
async fn diff(
    State(state): State<Arc<ControlState>>,
    Path(number): Path<u64>,
) -> Result<Json<Value>, HttpError> {
    let source = state.policy.source.clone();
    run_read(state, move |objects| {
        objects.diff(number, &source).map(|diff| serde_json::json!({"diff":diff}))
    })
    .await
    .map(Json)
}
async fn receipt(
    State(state): State<Arc<ControlState>>,
    Path(request_id): Path<String>,
) -> Result<Json<ControlReceipt>, HttpError> {
    run_read(state, move |objects| {
        objects.control_receipt(&request_id)?.context("control receipt does not exist")
    })
    .await
    .map(Json)
}
async fn command(
    State(state): State<Arc<ControlState>>,
    request: Result<Json<CommandRequest>, JsonRejection>,
) -> Result<Response, HttpError> {
    let request = request
        .map_err(|error| HttpError {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            error: CommandFailure { code: "invalid_request".into(), message: error.body_text() },
            request_id: None,
        })?
        .0;
    let request_id = request.request_id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let span=tracing::info_span!("control.command",request_id=%request.request_id,operation=request.command.operation(),target_number=?request.command.target(),expected_revision=?request.expected_revision);
        let _entered=span.enter();
        let started=std::time::Instant::now();
        let _command=state.commands.lock().map_err(|e|CommandFailure{code:"unavailable".into(),message:format!("control command owner lock failed: {e}")})?;
        let mut objects = LocalObjects::open(&state.database, &state.repository).map_err(|e| {
            CommandFailure { code: "unavailable".into(), message: format!("{e:#}") }
        })?;
        let result=objects.apply_request(&request, &state.policy);
        match &result {
            Ok(receipt)=>tracing::info!(target:"braid.control",request_id=%request.request_id,operation=request.command.operation(),target_number=?request.command.target(),expected_revision=?request.expected_revision,receipt_state=%receipt.state,error=?receipt.error,elapsed_seconds=started.elapsed().as_secs_f64(),"control command settled"),
            Err(error)=>tracing::error!(target:"braid.control",request_id=%request.request_id,operation=request.command.operation(),target_number=?request.command.target(),expected_revision=?request.expected_revision,error=%error,elapsed_seconds=started.elapsed().as_secs_f64(),"control command rejected"),
        }
        result
    })
    .await
    .map_err(HttpError::internal)?;
    let receipt = result.map_err(|error| HttpError::command(error, Some(request_id)))?;
    let status = match receipt.state.as_str() {
        "prepared" => StatusCode::ACCEPTED,
        "applied" => StatusCode::OK,
        _ => StatusCode::CONFLICT,
    };
    Ok((status, Json(receipt)).into_response())
}

async fn run_read<T: Send + 'static>(
    state: Arc<ControlState>,
    read: impl FnOnce(&LocalObjects) -> Result<T> + Send + 'static,
) -> Result<T, HttpError> {
    tokio::task::spawn_blocking(move || {
        let objects = LocalObjects::open_read_only(&state.database, &state.repository)?;
        objects.verify_source(&state.policy.source)?;
        read(&objects)
    })
    .await
    .map_err(HttpError::internal)?
    .map_err(HttpError::internal)
}

struct HttpError {
    status: StatusCode,
    error: CommandFailure,
    request_id: Option<String>,
}
impl HttpError {
    fn command(error: CommandFailure, request_id: Option<String>) -> Self {
        let status = if error.code == "conflict" {
            StatusCode::CONFLICT
        } else {
            StatusCode::UNPROCESSABLE_ENTITY
        };
        Self { status, error, request_id }
    }
    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            error: CommandFailure { code: "unavailable".into(), message: error.to_string() },
            request_id: None,
        }
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.status, Json(serde_json::json!({"error":self.error,"requestId":self.request_id})))
            .into_response()
    }
}
