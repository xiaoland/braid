use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State, rejection::BytesRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use prost::Message;
use serde_json::{Value, json};

use super::{
    collector,
    storage::{Filter, Storage},
};

#[derive(Clone)]
pub struct ApiState {
    pub storage: Arc<Mutex<Storage>>,
}

pub fn routes(state: ApiState) -> Router {
    Router::new()
        .route("/v1/traces", post(traces).fallback(otlp_method_not_allowed))
        .route("/v1/logs", post(logs).fallback(otlp_method_not_allowed))
        .route("/v1/metrics", post(metrics).fallback(otlp_method_not_allowed))
        .route("/api/resources", get(resources))
        .route("/api/status", get(resources))
        .route("/api/records", get(records))
        .route("/api/batches/{id}", get(batch))
        .route("/api/receive-errors", get(receive_errors))
        .route("/api/receive-errors/{id}", get(rejected_body))
        .with_state(state)
}

#[derive(prost::Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

pub(super) fn otlp_failure(status: StatusCode, message: String) -> Response {
    let code = match status {
        StatusCode::SERVICE_UNAVAILABLE => 14,
        StatusCode::INTERNAL_SERVER_ERROR => 13,
        StatusCode::FORBIDDEN => 7,
        StatusCode::METHOD_NOT_ALLOWED => 12,
        _ => 3,
    };
    (
        status,
        [("content-type", "application/x-protobuf")],
        RpcStatus { code, message }.encode_to_vec(),
    )
        .into_response()
}

async fn otlp_method_not_allowed() -> Response {
    otlp_failure(StatusCode::METHOD_NOT_ALLOWED, "OTLP export requires POST".into())
}

struct OtlpError(ApiError);
impl IntoResponse for OtlpError {
    fn into_response(self) -> Response {
        otlp_failure(self.0.0, self.0.1)
    }
}

struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

async fn traces(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, OtlpError> {
    ingest(state, headers, body, "traces").await.map_err(OtlpError)
}
async fn logs(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, OtlpError> {
    ingest(state, headers, body, "logs").await.map_err(OtlpError)
}
async fn metrics(
    State(state): State<ApiState>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, OtlpError> {
    ingest(state, headers, body, "metrics").await.map_err(OtlpError)
}

async fn ingest(
    state: ApiState,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
    signal: &'static str,
) -> Result<Response, ApiError> {
    let body = match body {
        Ok(body) => body,
        Err(rejection) => {
            let status = rejection.status();
            let reason = format!(
                "{}; request body was not captured because extraction failed",
                rejection.body_text()
            );
            let mut error = ApiError(status, reason);
            let persisted = tokio::task::spawn_blocking({
                let state = state.clone();
                let reason = error.1.clone();
                move || {
                    state
                        .storage
                        .lock()
                        .map_err(internal)?
                        .reject(signal, status.as_u16(), &reason, &[])
                        .map_err(internal)
                }
            })
            .await
            .map_err(internal)?;
            if let Err(persist) = persisted {
                error.1 = format!("{}; receive-error persistence failed: {}", error.1, persist.1);
            }
            return Err(error);
        }
    };
    tokio::task::spawn_blocking(move || {
        let validated = validate(&headers, &body, signal);
        let mut storage = state.storage.lock().map_err(internal)?;
        let outcome = match validated {
            Ok((encoding, records)) => storage
                .ingest(signal, &encoding, &body, records)
                .map_err(|error| ApiError(StatusCode::SERVICE_UNAVAILABLE, format!("{error:#}"))),
            Err(error) => Err(error),
        };
        if let Err(mut error) = outcome {
            if let Err(persist) = storage.reject(signal, error.0.as_u16(), &error.1, &body) {
                error.1 = format!("{}; receive-error persistence failed: {persist:#}", error.1);
            }
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(internal)??;
    Ok(([("content-type", "application/x-protobuf")], Bytes::new()).into_response())
}

fn validate(
    headers: &HeaderMap,
    body: &[u8],
    signal: &str,
) -> Result<(String, Vec<super::storage::DecodedRecord>), ApiError> {
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if content_type != "application/x-protobuf" {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "OTLP requires Content-Type: application/x-protobuf".into(),
        ));
    }
    let encoding = headers
        .get("content-encoding")
        .map(|v| v.to_str())
        .transpose()
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, e.to_string()))?
        .unwrap_or("identity")
        .to_owned();
    if !matches!(encoding.as_str(), "gzip" | "identity") {
        return Err(ApiError(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("unsupported Content-Encoding: {encoding}"),
        ));
    }
    let records = collector::decode(signal, &encoding, body)
        .map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    Ok((encoding, records))
}

async fn receive_errors(
    State(state): State<ApiState>,
    Query(filter): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
    tokio::task::spawn_blocking(move || {
        state.storage.lock().map_err(internal)?.receive_errors(&filter).map(Json).map_err(internal)
    })
    .await
    .map_err(internal)?
}
async fn rejected_body(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    let body = tokio::task::spawn_blocking(move || {
        state.storage.lock().map_err(internal)?.rejected_body(id).map_err(internal)
    })
    .await
    .map_err(internal)??;
    let Some(body) = body else {
        return Err(ApiError(StatusCode::NOT_FOUND, format!("receive error {id} not found")));
    };
    let mut response = ([("content-type", "application/octet-stream")], body).into_response();
    response.headers_mut().insert(
        "content-disposition",
        axum::http::HeaderValue::from_str(&format!(
            "attachment; filename=otlp-receive-error-{id}.bin"
        ))
        .map_err(internal)?,
    );
    Ok(response)
}

async fn resources(State(state): State<ApiState>) -> Result<Json<Value>, ApiError> {
    tokio::task::spawn_blocking(move || {
        state.storage.lock().map_err(internal)?.resources().map(Json).map_err(internal)
    })
    .await
    .map_err(internal)?
}
async fn records(
    State(state): State<ApiState>,
    Query(filter): Query<Filter>,
) -> Result<Json<Value>, ApiError> {
    if filter.signal.as_ref().is_some_and(|s| !matches!(s.as_str(), "traces" | "logs" | "metrics"))
    {
        return Err(ApiError(
            StatusCode::BAD_REQUEST,
            "signal must be traces, logs, or metrics".into(),
        ));
    }
    if filter.after.is_some_and(|id| id < 0) {
        return Err(ApiError(StatusCode::BAD_REQUEST, "after must be nonnegative".into()));
    }
    tokio::task::spawn_blocking(move || {
        state.storage.lock().map_err(internal)?.records(&filter).map(Json).map_err(internal)
    })
    .await
    .map_err(internal)?
}
async fn batch(State(state): State<ApiState>, Path(id): Path<i64>) -> Result<Response, ApiError> {
    let found = tokio::task::spawn_blocking(move || {
        state.storage.lock().map_err(internal)?.batch(id).map_err(internal)
    })
    .await
    .map_err(internal)??;
    let Some((encoding, body)) = found else {
        return Err(ApiError(StatusCode::NOT_FOUND, format!("OTLP batch {id} not found")));
    };
    let mut response = ([("content-type", "application/octet-stream")], body).into_response();
    let suffix = if encoding == "gzip" { "pb.gz" } else { "pb" };
    response.headers_mut().insert(
        "content-disposition",
        axum::http::HeaderValue::from_str(&format!(
            "attachment; filename=otlp-batch-{id}.{suffix}"
        ))
        .map_err(internal)?,
    );
    response.headers_mut().insert(
        "x-otlp-content-encoding",
        axum::http::HeaderValue::from_str(&encoding).map_err(internal)?,
    );
    Ok(response)
}
