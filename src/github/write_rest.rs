//! REST publication shares installation cooldown, not GraphQL's point accounting.
use super::rate_limit;
use super::{DeserializeOwned, GitHubClient, GitHubError, Serialize, bounded, github_step};

impl GitHubClient {
    pub(super) async fn write_rest<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        method: &str,
        path: &str,
        body: Option<&B>,
    ) -> Result<T, GitHubError> {
        let gate = self.rate_limits.installation(self.identity.installation_id);
        let mut budget = gate.lock().await;
        if let Err(message) = budget.admit(rate_limit::now(), self.background) {
            return Err(GitHubError::RateLimited {
                retry_after: budget.retry_after(rate_limit::now()),
                message,
            });
        }
        let result = github_step(path, async {
            let response = match method {
                "GET" => self.installation._get(path).await?,
                "POST" => self.installation._post(path, body).await?,
                "PATCH" => self.installation._patch(path, body).await?,
                _ => unreachable!("publication REST method"),
            };
            let status = response.status();
            let number = |name| {
                response
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
            };
            let retry_after = number("retry-after");
            let exhausted = number("x-ratelimit-remaining") == Some(0);
            let delay = retry_after.unwrap_or(0).max(if exhausted {
                number("x-ratelimit-reset").unwrap_or(0).saturating_sub(rate_limit::now())
            } else {
                0
            });
            let header_limit = status == axum::http::StatusCode::TOO_MANY_REQUESTS
                || status == axum::http::StatusCode::FORBIDDEN
                    && (retry_after.is_some() || exhausted);
            // Retain server guidance even if reading the body fails or times out.
            if header_limit || exhausted || status.is_server_error() {
                budget.defer(rate_limit::now(), (delay > 0).then_some(delay));
            }
            let body = self.installation.body_to_string(response).await?;
            if !status.is_success() {
                let error = GitHubError::Http { status, body: bounded(&body, 1024).to_owned() };
                if header_limit || error.is_rate_limited() {
                    if !header_limit {
                        budget.defer(rate_limit::now(), retry_after);
                    }
                    return Err(GitHubError::RateLimited {
                        retry_after: budget.retry_after(rate_limit::now()),
                        message: error.to_string(),
                    });
                }
                return Err(error);
            }
            let value = serde_json::from_str(&body)
                .map_err(|error| GitHubError::Octocrab(error.to_string()))?;
            if !exhausted {
                budget.success();
            }
            Ok(value)
        })
        .await;
        if result.as_ref().is_err_and(GitHubError::is_unavailable) {
            budget.defer(rate_limit::now(), None);
        }
        result
    }
}
