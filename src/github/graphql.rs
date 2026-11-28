//! Raw GraphQL transport: response accounting and installation admission live together.
use super::{
    error::{GitHubError, bounded},
    rate_limit::{self, RateLimits},
};
use crate::telemetry::github as observation;
use axum::http::StatusCode;
use octocrab::{FromResponse, Octocrab};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::time::{Duration, Instant};

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
    let operation = operation(query);
    let class = if background { "background" } else { "foreground" };
    let started = Instant::now();
    let mut observed_cost = None;
    let mut response_metadata = None;
    let gate = rate_limits.installation(installation_id);
    // Hold admission through the response: after a limit is confirmed no queued
    // repository request can race past it. No sleeps or internal retry loops.
    let mut budget = gate.lock().await;
    if let Err(reason) = budget.admit(rate_limit::now(), background) {
        tracing::warn!(installation = installation_id, operation, class, resource = "graphql", %reason, "GitHub GraphQL delayed");
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
        let request_id = response
            .headers()
            .get("x-github-request-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_owned();
        let resource = response
            .headers()
            .get("x-ratelimit-resource")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_owned();
        response_metadata =
            Some((status, resource.clone(), request_id.clone(), used_delta, retry_after));
        tracing::debug!(operation, class, resource, request_id, latency_ms = started.elapsed().as_millis(), installation = installation_id, request = budget.requests,
                used = ?budget.used, header_used_delta = ?used_delta, remaining = ?budget.remaining,
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
        let envelope: GraphQlEnvelope<ObservedData<T>> =
            FromResponse::from_response(response).await?;
        observed_cost = envelope
            .data
            .as_ref()
            .and_then(|data| data.rate_limit.as_ref().map(|limit| limit.cost));
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
        Ok(data.value)
    };
    let result = tokio::time::timeout(Duration::from_secs(30), request).await;
    let result = match result {
        Ok(Err(error @ GitHubError::Octocrab(_))) => {
            budget.defer(rate_limit::now(), None);
            Err(error)
        }
        Ok(result) => result,
        Err(_) => {
            budget.defer(rate_limit::now(), None);
            Err(GitHubError::Timeout { operation: "GraphQL request".into() })
        }
    };
    let outcome = match &result {
        Ok(_) => "success",
        Err(GitHubError::Delayed(_)) => "limited",
        Err(_) => "failed",
    };
    observation::observe(operation, class, outcome, started.elapsed().as_secs_f64(), observed_cost);
    if result.is_err() {
        tracing::warn!(operation, class, installation = installation_id, resource = "graphql", response = ?response_metadata, graphql_cost = ?observed_cost, used = ?budget.used, outcome, latency_ms = started.elapsed().as_millis(), remaining = ?budget.remaining, reset = ?budget.reset, "GitHub GraphQL request failed; payload excluded");
    } else {
        tracing::debug!(operation, class, installation = installation_id, graphql_cost = ?observed_cost, "GitHub GraphQL operation complete (None cost is unknown)");
    }
    result
}

#[derive(Deserialize)]
struct ObservedData<T> {
    #[serde(rename = "rateLimit")]
    rate_limit: Option<QueryCost>,
    #[serde(flatten)]
    value: T,
}
#[derive(Deserialize)]
struct QueryCost {
    cost: u64,
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
struct GraphQlEnvelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
    #[serde(rename = "type", default)]
    kind: Option<String>,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    message: Option<String>,
}

fn operation(query: &str) -> &'static str {
    let name = query
        .trim_start()
        .strip_prefix("query ")
        .and_then(|q| q.split(['(', '{']).next())
        .map(str::trim);
    match name {
        Some("IssueCore") => "IssueCore",
        Some("PrCore") => "PrCore",
        Some("IssueAssignees") => "IssueAssignees",
        Some("IssueLabels") => "IssueLabels",
        Some("IssueProjects") => "IssueProjects",
        Some("PrProjects") => "PrProjects",
        Some("ProjectFieldValues") => "ProjectFieldValues",
        Some("IssueLinkedBranches") => "IssueLinkedBranches",
        Some("IssueSubIssues") => "IssueSubIssues",
        Some("IssueBlockedBy") => "IssueBlockedBy",
        Some("IssueBlocking") => "IssueBlocking",
        Some("IssueAssociatedPrs") => "IssueAssociatedPrs",
        Some("IssueComments") => "IssueComments",
        Some("IssueDuplicates") => "IssueDuplicates",
        Some("PrAssignees") => "PrAssignees",
        Some("PrLabels") => "PrLabels",
        Some("PrAssociatedIssues") => "PrAssociatedIssues",
        Some("PrComments") => "PrComments",
        Some("PrReviews") => "PrReviews",
        Some("PrReviewThreads") => "PrReviewThreads",
        Some("ReviewThreadComments") => "ReviewThreadComments",
        Some("IssueFirstPages") => "IssueFirstPages",
        Some("PrFirstPages") => "PrFirstPages",
        Some("RepositoryIdentity") => "RepositoryIdentity",
        Some("ViewerIdentity") => "ViewerIdentity",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::{GraphQlEnvelope, ObservedData, operation};
    use serde_json::{Value, json};

    #[test]
    fn returned_cost_is_separate_from_data_and_missing_cost_is_unknown() {
        let envelope: GraphQlEnvelope<ObservedData<Value>> = serde_json::from_value(json!({
            "data":{"rateLimit":{"cost":7},"answer":42}
        }))
        .unwrap();
        let data = envelope.data.unwrap();
        assert_eq!(data.rate_limit.unwrap().cost, 7);
        assert_eq!(data.value, json!({"answer":42}));
        let envelope: GraphQlEnvelope<ObservedData<Value>> =
            serde_json::from_value(json!({"data":{"answer":42}})).unwrap();
        assert!(envelope.data.unwrap().rate_limit.is_none());
    }
    #[test]
    fn operation_labels_are_bounded_and_do_not_copy_arbitrary_query_names() {
        assert_eq!(operation("query IssueFirstPages($id:ID!){repository{id}}"), "IssueFirstPages");
        assert_eq!(operation("query repo_secret_123{viewer{id}}"), "other");
        assert_eq!(operation("query{viewer{id}}"), "other");
    }
}
