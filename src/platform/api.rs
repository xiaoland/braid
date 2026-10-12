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
    platform::config::PlatformConfig,
    platform::objects::{
        CommandFailure, CommandRequest, ControlPolicy, ControlReceipt, PlatformObjects,
    },
};

struct ControlState {
    database: std::path::PathBuf,
    repository: String,
    policy: ControlPolicy,
    commands: Mutex<()>,
}

pub fn router(config: &PlatformConfig) -> Result<Router> {
    super::store::initialize(config)?;
    PlatformObjects::open(&config.database, &config.repository)?
        .pin_existing_candidates(&config.source)?;
    let state = Arc::new(ControlState {
        database: config.database.clone(),
        repository: config.repository.clone(),
        commands: Mutex::new(()),
        policy: ControlPolicy { source: config.source.clone() },
    });
    Ok(Router::new()
        .route("/api/control/snapshot", get(snapshot))
        .route("/api/control/items", get(items))
        .route("/api/platform/repository", get(repository))
        .route("/api/platform/context/{number}", get(context))
        .route("/api/platform/scope/{number}", get(scope))
        .route("/api/platform/events", get(events))
        .route("/api/platform/writes", post(writes))
        .route("/api/platform/grants", post(grant))
        .route("/api/platform/grants/revoke", post(revoke))
        .route("/api/control/items/{number}", get(item))
        .route("/api/control/items/{number}/diff", get(diff))
        .route("/api/control/receipts/{request_id}", get(receipt))
        .route("/api/control/commands", post(command))
        .layer(axum::middleware::from_fn_with_state(config.listen.clone(), loopback_gate))
        .with_state(state))
}

async fn snapshot(State(state): State<Arc<ControlState>>) -> Result<Json<Value>, HttpError> {
    run_read(state, PlatformObjects::control_snapshot).await.map(Json)
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
    run_read(state, move |objects| objects.diff(number, &source)).await.map(Json)
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
        let mut objects = PlatformObjects::open(&state.database, &state.repository).map_err(|e| {
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
    read: impl FnOnce(&PlatformObjects) -> Result<T> + Send + 'static,
) -> Result<T, HttpError> {
    tokio::task::spawn_blocking(move || {
        let objects = PlatformObjects::open_read_only(&state.database, &state.repository)?;
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

async fn items(State(state): State<Arc<ControlState>>) -> Result<Json<Value>, HttpError> {
    run_read(state, |o| Ok(serde_json::to_value(o.list()?)?)).await.map(Json)
}
async fn repository(State(state): State<Arc<ControlState>>) -> Result<Json<Value>, HttpError> {
    run_read(state, PlatformObjects::repository_info).await.map(Json)
}
async fn context(
    State(state): State<Arc<ControlState>>,
    Path(number): Path<u64>,
) -> Result<Json<Value>, HttpError> {
    run_read(state, move |o| Ok(serde_json::to_value(o.context(number)?)?)).await.map(Json)
}
#[derive(serde::Deserialize)]
struct EventQuery {
    #[serde(default)]
    after: u64,
    #[serde(default = "event_limit")]
    limit: u32,
}
fn event_limit() -> u32 {
    100
}
async fn events(
    State(state): State<Arc<ControlState>>,
    axum::extract::Query(query): axum::extract::Query<EventQuery>,
) -> Result<Json<Value>, HttpError> {
    run_read(state, move |o| o.event_page(query.after, query.limit)).await.map(Json)
}
async fn writes(
    State(state): State<Arc<ControlState>>,
    Json(request): Json<super::writes::WriteRequest>,
) -> Result<Json<Value>, HttpError> {
    tokio::task::spawn_blocking(move || {
        let mut o = PlatformObjects::open(&state.database, &state.repository)?;
        o.apply_outbox(&request).map(
            |(database_id, node_id)| serde_json::json!({"databaseId":database_id,"nodeId":node_id}),
        )
    })
    .await
    .map_err(HttpError::internal)?
    .map(Json)
    .map_err(HttpError::internal)
}
async fn grant(
    State(state): State<Arc<ControlState>>,
    Json(request): Json<super::grants::GrantRequest>,
) -> Result<Json<Value>, HttpError> {
    grant_change(state, request, false).await
}
async fn revoke(
    State(state): State<Arc<ControlState>>,
    Json(request): Json<super::grants::GrantRequest>,
) -> Result<Json<Value>, HttpError> {
    grant_change(state, request, true).await
}
async fn grant_change(
    state: Arc<ControlState>,
    request: super::grants::GrantRequest,
    revoke: bool,
) -> Result<Json<Value>, HttpError> {
    tokio::task::spawn_blocking(move || super::grants::change(&state.database, &request, revoke))
        .await
        .map_err(HttpError::internal)?
        .map(Json)
        .map_err(|e| HttpError {
            status: StatusCode::CONFLICT,
            error: CommandFailure { code: "conflict".into(), message: format!("{e:#}") },
            request_id: None,
        })
}
async fn loopback_gate(
    State(expected): State<String>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let authority = request.headers().get(axum::http::header::HOST).and_then(|h| h.to_str().ok());
    let allowed = authority == Some(expected.as_str());
    let origin_ok = request.headers().get(axum::http::header::ORIGIN).is_none_or(|origin| {
        origin.to_str().ok().is_some_and(|origin| origin == format!("http://{expected}"))
    });
    if !allowed || !origin_ok {
        return (StatusCode::FORBIDDEN,Json(serde_json::json!({"error":{"code":"forbidden","message":"platform Host/Origin must match its declared loopback listener"}}))).into_response();
    }
    next.run(request).await
}

async fn scope(
    State(state): State<Arc<ControlState>>,
    Path(number): Path<u64>,
) -> Result<Json<super::objects::ScopeSnapshot>, HttpError> {
    run_read(state, move |o| o.scope(number)).await.map(Json)
}
