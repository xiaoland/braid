use axum::http::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum GitHubError {
    #[error("invalid GitHub repository {0:?}; expected owner/name")]
    InvalidRepository(String),
    #[error("invalid GitHub work item {0:?}; expected owner/name#number")]
    InvalidWorkItem(String),
    #[error("cannot read GitHub App private key: {0}")]
    PrivateKey(#[from] std::io::Error),
    #[error("GitHub App private key is not a usable RSA PEM: {0}")]
    InvalidPrivateKey(jsonwebtoken::errors::Error),
    #[error("cannot build GitHub client: {0}")]
    Client(String),
    #[error("GitHub returned {status}: {body}")]
    Http { status: StatusCode, body: String },
    #[error("GitHub GraphQL returned errors: {0}")]
    GraphQl(String),
    #[error("ambiguous GitHub write: {0}")]
    Ambiguous(String),
    #[error("GitHub REST rate limited ({retry_after}s): {message}")]
    RateLimited { retry_after: u64, message: String },
    #[error("GitHub API delayed: {0}")]
    Delayed(String),
    #[error("GitHub GraphQL returned no data")]
    MissingData,
    #[error("GitHub state has not converged yet: {0}")]
    ConvergencePending(String),
    #[error("credential resolved App {actual}, expected {expected}")]
    WrongApp { actual: u64, expected: u64 },
    #[error("GitHub App installation lacks required permissions: {0}")]
    InsufficientPermissions(String),
    #[error("GitHub client error: {0}")]
    Octocrab(String),
    #[error("GitHub {operation} timed out after 30s (network unavailable or API unreachable)")]
    Timeout { operation: String },
}

impl From<octocrab::Error> for GitHubError {
    fn from(error: octocrab::Error) -> Self {
        match error {
            octocrab::Error::GitHub { source, .. } => Self::Http {
                status: source.status_code,
                body: bounded(&source.message, 1024).to_owned(),
            },
            _ => Self::Octocrab(error.to_string()),
        }
    }
}

impl GitHubError {
    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Delayed(_) | Self::RateLimited { .. })
            || matches!(self, Self::Http {status, body} if *status == StatusCode::TOO_MANY_REQUESTS || (*status == StatusCode::FORBIDDEN && { let message=body.to_ascii_lowercase(); message.contains("rate limit") || message.contains("secondary rate") }))
    }

    pub fn is_unavailable(&self) -> bool {
        self.is_rate_limited()
            || matches!(
                self,
                Self::Octocrab(_)
                    | Self::ConvergencePending(_)
                    | Self::Timeout { .. }
                    | Self::Delayed(_)
            )
            || matches!(self, Self::Http { status, .. }
            if status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS)
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Http { status, .. } if *status == StatusCode::UNPROCESSABLE_ENTITY || *status == StatusCode::CONFLICT)
    }
}

pub(super) fn bounded(value: &str, bytes: usize) -> &str {
    if value.len() <= bytes {
        return value;
    }
    let mut end = bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
