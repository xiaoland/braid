//! Raw GraphQL transport: response accounting and installation admission live together.
use super::{
    error::{GitHubError, bounded},
    rate_limit::{self, RateLimits},
};
use axum::http::StatusCode;
use octocrab::{FromResponse, Octocrab};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::Duration;

/// # Errors
/// Returns delayed admission, bounded transport failures, or `GitHub` response errors.
#[allow(clippy::too_many_lines)]
pub async fn request<V: Serialize + ?Sized, T: DeserializeOwned>(
    client: &Octocrab,
    rate_limits: &RateLimits,
    installation_id: u64,
    background: bool,
    query: &str,
    variables: &V,
) -> Result<T, GitHubError> {
    let gate = rate_limits.installation(installation_id);
    // Hold admission through the response: after a limit is confirmed no queued
    // repository request can race past it. No sleeps or internal retry loops.
    let mut budget = gate.lock().await;
    if let Err(reason) = budget.admit(rate_limit::now(), background) {
        tracing::warn!(installation = installation_id, %reason, "GitHub GraphQL delayed");
        return Err(GitHubError::Delayed(reason));
    }
    let request = async {
        let response = client._post("/graphql", Some(&GraphQlRequest { query, variables })).await?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let used_delta = budget.observe(response.headers());
        tracing::info!(installation = installation_id, request = budget.requests,
                used = ?budget.used, cost_or_used_delta = ?used_delta, remaining = ?budget.remaining,
                reset = ?budget.reset, %status, "GitHub GraphQL response budget");
        if !status.is_success() {
            // Honor limit headers before decoding: a secondary limit may
            // return HTML or stall while streaming its error body.
            let header_limit = status == StatusCode::TOO_MANY_REQUESTS
                || status == StatusCode::FORBIDDEN
                    && (retry_after.is_some() || budget.remaining == Some(0));
            if header_limit || status.is_server_error() {
                budget.defer(rate_limit::now(), retry_after);
            }
            let body = ApiErrorBody::from_response(response)
                .await
                .ok()
                .and_then(|body| body.message)
                .unwrap_or_else(|| {
                    "GraphQL request rejected; see response budget telemetry".into()
                });
            let limited =
                header_limit || status == StatusCode::FORBIDDEN && rate_limit_message(&body);
            if limited && !header_limit {
                budget.defer(rate_limit::now(), retry_after);
            }
            if limited {
                // The first rejected write must be retryable too, not only
                // subsequent requests refused by the local cooldown gate.
                return Err(GitHubError::Delayed(format!(
                    "installation {installation_id}: HTTP {status}: {}",
                    bounded(&body, 1024)
                )));
            }
            return Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() });
        }
        let envelope: GraphQlEnvelope<T> = FromResponse::from_response(response).await?;
        if !envelope.errors.is_empty() {
            let limited = envelope.errors.iter().any(|error| {
                error.kind.as_deref().is_some_and(|kind| kind.eq_ignore_ascii_case("RATE_LIMITED"))
                    || rate_limit_message(&error.message)
            }) || retry_after.is_some()
                || budget.remaining == Some(0);
            let messages = envelope
                .errors
                .into_iter()
                .map(|error| {
                    error
                        .kind
                        .map_or(error.message.clone(), |kind| format!("{kind}: {}", error.message))
                })
                .collect::<Vec<_>>()
                .join("; ");
            if limited {
                budget.defer(rate_limit::now(), retry_after);
                return Err(GitHubError::Delayed(format!(
                    "installation {installation_id}: {}",
                    bounded(&messages, 2048)
                )));
            }
            return Err(GitHubError::GraphQl(bounded(&messages, 2048).to_owned()));
        }
        let data = envelope.data.ok_or(GitHubError::MissingData)?;
        budget.success();
        if budget.remaining == Some(0) {
            budget.defer(rate_limit::now(), None);
        }
        Ok(data)
    };
    let result = tokio::time::timeout(Duration::from_secs(30), request).await;
    match result {
        Ok(Err(error @ GitHubError::Octocrab(_))) => {
            budget.defer(rate_limit::now(), None);
            Err(error)
        }
        Ok(result) => result,
        Err(_) => {
            budget.defer(rate_limit::now(), None);
            Err(GitHubError::Timeout { operation: "GraphQL request".into() })
        }
    }
}

// Some GitHub responses omit a structured type (including HTTP 403 bodies).
// Keep this compatibility check at the API boundary, not in worker policies.
fn rate_limit_message(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit") || message.contains("rate_limit") || message.contains("abuse")
}

#[derive(Serialize)]
struct GraphQlRequest<'a, V: ?Sized> {
    query: &'a str,
    variables: &'a V,
}

#[derive(Deserialize)]
pub(super) struct GraphQlEnvelope<T> {
    pub(super) data: Option<T>,
    #[serde(default)]
    pub(super) errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
pub(super) struct GraphQlError {
    pub(super) message: String,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: Option<String>,
}
