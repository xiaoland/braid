use std::{collections::BTreeMap, fmt, fs, str::FromStr, time::Duration};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;
use time::OffsetDateTime;

use crate::config::GitHubConfig;

const API_ROOT: &str = "https://api.github.com";
const GRAPHQL_URL: &str = "https://api.github.com/graphql";

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
    #[error("cannot sign GitHub App JWT: {0}")]
    Jwt(jsonwebtoken::errors::Error),
    #[error("cannot build GitHub HTTP client: {0}")]
    Client(reqwest::Error),
    #[error("GitHub request failed: {0}")]
    Transport(reqwest::Error),
    #[error("GitHub returned {status}: {body}")]
    Http { status: StatusCode, body: String },
    #[error("GitHub response shape is unsupported: {0}")]
    Response(reqwest::Error),
    #[error("GitHub GraphQL returned errors: {0}")]
    GraphQl(String),
    #[error("GitHub GraphQL returned no data")]
    MissingData,
    #[error("GitHub state has not converged yet: {0}")]
    ConvergencePending(String),
    #[error("credential resolved App {actual}, expected {expected}")]
    WrongApp { actual: u64, expected: u64 },
    #[error("GitHub App installation lacks required permissions: {0}")]
    InsufficientPermissions(String),
}

impl GitHubError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Transport(_) | Self::ConvergencePending(_))
            || matches!(self, Self::Http { status, .. } if status.is_server_error())
    }

    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::Http { status, .. } if *status == StatusCode::UNPROCESSABLE_ENTITY || *status == StatusCode::CONFLICT)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RepositoryName {
    pub owner: String,
    pub name: String,
}

impl RepositoryName {
    pub fn name_with_owner(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.owner, self.name)
    }
}

impl FromStr for RepositoryName {
    type Err = GitHubError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (owner, name) = value
            .split_once('/')
            .ok_or_else(|| GitHubError::InvalidRepository(value.to_owned()))?;
        if owner.is_empty()
            || name.is_empty()
            || name.contains('/')
            || !owner.chars().all(valid_name_character)
            || !name.chars().all(valid_name_character)
        {
            return Err(GitHubError::InvalidRepository(value.to_owned()));
        }
        Ok(Self { owner: owner.to_owned(), name: name.to_owned() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct WorkItemLocator {
    pub repository: RepositoryName,
    pub number: u64,
}

impl fmt::Display for WorkItemLocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}#{}", self.repository, self.number)
    }
}

impl FromStr for WorkItemLocator {
    type Err = GitHubError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (repository, number) =
            value.rsplit_once('#').ok_or_else(|| GitHubError::InvalidWorkItem(value.to_owned()))?;
        let repository =
            repository.parse().map_err(|_| GitHubError::InvalidWorkItem(value.to_owned()))?;
        let number = number
            .parse::<u64>()
            .ok()
            .filter(|number| *number > 0)
            .ok_or_else(|| GitHubError::InvalidWorkItem(value.to_owned()))?;
        Ok(Self { repository, number })
    }
}

fn valid_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

#[derive(Debug, Clone, Serialize)]
pub struct GitHubIdentity {
    pub app_id: u64,
    pub app_slug: String,
    pub installation_id: u64,
    pub repository: String,
    pub repository_node_id: String,
    pub actor_node_id: String,
    pub actor_login: String,
    pub token_expires_at: String,
    pub permissions: BTreeMap<String, String>,
}

pub struct GitHubClient {
    http: Client,
    config: GitHubConfig,
    api_version: String,
    installation_token: String,
    identity: GitHubIdentity,
    projects_v2_enabled: bool,
}

impl GitHubClient {
    pub async fn connect(
        config: &GitHubConfig,
        repository: &RepositoryName,
    ) -> Result<Self, GitHubError> {
        let http = Client::builder()
            .user_agent(format!("braid/{}", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(30))
            .tls_backend_rustls()
            .build()
            .map_err(GitHubError::Client)?;
        let app_jwt = app_jwt(config)?;
        let app = rest_get::<AppResponse>(&http, "/app", &app_jwt, &config.api_version).await?;
        if app.id != config.app_id {
            return Err(GitHubError::WrongApp { actual: app.id, expected: config.app_id });
        }
        let installation = rest_get::<InstallationResponse>(
            &http,
            &format!("/repos/{repository}/installation"),
            &app_jwt,
            &config.api_version,
        )
        .await?;
        let access = rest_post_empty::<AccessTokenResponse>(
            &http,
            &format!("/app/installations/{}/access_tokens", installation.id),
            &app_jwt,
            &config.api_version,
        )
        .await?;
        let repository_info = repository_identity(&http, &access.token, repository).await?;
        let actor = viewer_identity(&http, &access.token).await?;
        validate_read_permissions(&access.permissions, config.projects_v2_enabled)?;
        Ok(Self {
            http,
            config: config.clone(),
            api_version: config.api_version.clone(),
            installation_token: access.token,
            projects_v2_enabled: config.projects_v2_enabled,
            identity: GitHubIdentity {
                app_id: app.id,
                app_slug: app.slug,
                installation_id: installation.id,
                repository: repository_info.name_with_owner,
                repository_node_id: repository_info.id,
                actor_node_id: actor.id,
                actor_login: actor.login,
                token_expires_at: access.expires_at,
                permissions: access.permissions,
            },
        })
    }

    pub fn identity(&self) -> &GitHubIdentity {
        &self.identity
    }

    pub fn projects_v2_enabled(&self) -> bool {
        self.projects_v2_enabled
    }

    pub async fn for_repository(&self, repository: &RepositoryName) -> Result<Self, GitHubError> {
        Self::connect(&self.config, repository).await
    }

    pub async fn graphql<V, T>(&self, query: &str, variables: &V) -> Result<T, GitHubError>
    where
        V: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        let response = self
            .http
            .post(GRAPHQL_URL)
            .bearer_auth(&self.installation_token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", &self.api_version)
            .json(&GraphQlRequest { query, variables })
            .send()
            .await
            .map_err(GitHubError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() });
        }
        let envelope =
            response.json::<GraphQlEnvelope<T>>().await.map_err(GitHubError::Response)?;
        if !envelope.errors.is_empty() {
            let messages = envelope
                .errors
                .into_iter()
                .map(|error| error.message)
                .collect::<Vec<_>>()
                .join("; ");
            return Err(GitHubError::GraphQl(bounded(&messages, 2048).to_owned()));
        }
        envelope.data.ok_or(GitHubError::MissingData)
    }

    pub async fn repository_permission(&self, login: &str) -> Result<String, GitHubError> {
        let path = format!("/repos/{}/collaborators/{login}/permission", self.identity.repository);
        let permission = rest_get::<PermissionResponse>(
            &self.http,
            &path,
            &self.installation_token,
            &self.api_version,
        )
        .await?;
        Ok(permission.role_name.unwrap_or(permission.permission))
    }

    pub async fn add_reaction(
        &self,
        target_kind: &str,
        database_id: &str,
        content: &str,
    ) -> Result<u64, GitHubError> {
        let path = match target_kind {
            "issue_comment" => format!(
                "/repos/{}/issues/comments/{database_id}/reactions",
                self.identity.repository
            ),
            "review_comment" => format!(
                "/repos/{}/pulls/comments/{database_id}/reactions",
                self.identity.repository
            ),
            other => {
                return Err(GitHubError::GraphQl(format!("unsupported reaction target {other:?}")));
            }
        };
        let reaction: ReactionResponse = rest_post(
            &self.http,
            &path,
            &self.installation_token,
            &self.api_version,
            &ReactionRequest { content },
        )
        .await?;
        Ok(reaction.id)
    }

    pub async fn delete_reaction(
        &self,
        target_kind: &str,
        database_id: &str,
        reaction_id: u64,
    ) -> Result<(), GitHubError> {
        let path = match target_kind {
            "issue_comment" => format!(
                "/repos/{}/issues/comments/{database_id}/reactions/{reaction_id}",
                self.identity.repository
            ),
            "review_comment" => format!(
                "/repos/{}/pulls/comments/{database_id}/reactions/{reaction_id}",
                self.identity.repository
            ),
            other => {
                return Err(GitHubError::GraphQl(format!("unsupported reaction target {other:?}")));
            }
        };
        rest_delete(&self.http, &path, &self.installation_token, &self.api_version).await
    }

    pub async fn create_issue_comment(
        &self,
        issue_number: u64,
        body: &str,
    ) -> Result<CreatedIssueComment, GitHubError> {
        rest_post(
            &self.http,
            &format!("/repos/{}/issues/{issue_number}/comments", self.identity.repository),
            &self.installation_token,
            &self.api_version,
            &IssueCommentRequest { body },
        )
        .await
    }

    pub async fn issue_comments(
        &self,
        issue_number: u64,
    ) -> Result<Vec<IssueComment>, GitHubError> {
        let mut comments = Vec::new();
        for page in 1_u16..=100 {
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("per_page", "100")
                .append_pair("page", &page.to_string())
                .finish();
            let response = self
                .http
                .get(format!(
                    "{API_ROOT}/repos/{}/issues/{issue_number}/comments?{query}",
                    self.identity.repository
                ))
                .bearer_auth(&self.installation_token)
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", &self.api_version)
                .send()
                .await
                .map_err(GitHubError::Transport)?;
            let page_comments = rest_response::<Vec<IssueComment>>(response).await?;
            let complete = page_comments.len() < 100;
            comments.extend(page_comments);
            if complete {
                return Ok(comments);
            }
        }
        Err(GitHubError::GraphQl(
            "Issue comments exceed the bounded 10,000-comment write-recovery scan".into(),
        ))
    }

    pub async fn update_issue_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<CreatedIssueComment, GitHubError> {
        rest_patch(
            &self.http,
            &format!("/repos/{}/issues/comments/{comment_id}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
            &IssueCommentRequest { body },
        )
        .await
    }

    pub fn require_write_permissions(&self, permissions: &[&str]) -> Result<(), GitHubError> {
        let missing = permissions
            .iter()
            .filter(|name| {
                self.identity.permissions.get(**name).map(String::as_str) != Some("write")
            })
            .copied()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(GitHubError::InsufficientPermissions(
                missing
                    .into_iter()
                    .map(|name| format!("{name}:write"))
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
        }
    }

    pub async fn issue_comment(&self, comment_id: u64) -> Result<IssueComment, GitHubError> {
        rest_get(
            &self.http,
            &format!("/repos/{}/issues/comments/{comment_id}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
        )
        .await
    }

    pub async fn issue_or_pull_request(
        &self,
        number: u64,
    ) -> Result<IssueOrPullRequest, GitHubError> {
        rest_get(
            &self.http,
            &format!("/repos/{}/issues/{number}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
        )
        .await
    }

    pub async fn repository_details(&self) -> Result<RepositoryDetails, GitHubError> {
        rest_get(
            &self.http,
            &format!("/repos/{}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
        )
        .await
    }

    pub async fn git_reference(&self, name: &str) -> Result<Option<GitReference>, GitHubError> {
        let encoded = encode_path(name);
        rest_get_optional(
            &self.http,
            &format!("/repos/{}/git/ref/heads/{encoded}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
        )
        .await
    }

    pub async fn git_commit(&self, sha: &str) -> Result<GitCommit, GitHubError> {
        rest_get(
            &self.http,
            &format!("/repos/{}/git/commits/{sha}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
        )
        .await
    }

    pub async fn create_git_commit(
        &self,
        message: &str,
        tree: &str,
        parent: &str,
        authored_at: &str,
    ) -> Result<GitCommit, GitHubError> {
        let actor = format!("{}[bot]", self.identity.app_slug);
        let email = format!("{}+{}@users.noreply.github.com", self.identity.app_id, actor);
        rest_post(
            &self.http,
            &format!("/repos/{}/git/commits", self.identity.repository),
            &self.installation_token,
            &self.api_version,
            &CreateGitCommit {
                message,
                tree,
                parents: [parent],
                author: GitSignature { name: &actor, email: &email, date: authored_at },
                committer: GitSignature { name: &actor, email: &email, date: authored_at },
            },
        )
        .await
    }

    pub async fn create_git_reference(
        &self,
        name: &str,
        sha: &str,
    ) -> Result<GitReference, GitHubError> {
        let reference = format!("refs/heads/{name}");
        rest_post(
            &self.http,
            &format!("/repos/{}/git/refs", self.identity.repository),
            &self.installation_token,
            &self.api_version,
            &CreateGitReference { reference: &reference, sha },
        )
        .await
    }

    pub async fn update_git_reference(
        &self,
        name: &str,
        sha: &str,
    ) -> Result<GitReference, GitHubError> {
        let encoded = encode_path(name);
        rest_patch(
            &self.http,
            &format!("/repos/{}/git/refs/heads/{encoded}", self.identity.repository),
            &self.installation_token,
            &self.api_version,
            &UpdateGitReference { sha, force: false },
        )
        .await
    }

    pub async fn open_pull_requests_for_head(
        &self,
        head: &str,
        base: &str,
    ) -> Result<Vec<PullRequest>, GitHubError> {
        let head = format!(
            "{}:{head}",
            self.identity.repository.split_once('/').map_or("", |(owner, _)| owner)
        );
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("state", "open")
            .append_pair("head", &head)
            .append_pair("base", base)
            .append_pair("per_page", "100")
            .finish();
        let response = self
            .http
            .get(format!("{API_ROOT}/repos/{}/pulls?{query}", self.identity.repository))
            .bearer_auth(&self.installation_token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", &self.api_version)
            .send()
            .await
            .map_err(GitHubError::Transport)?;
        rest_response(response).await
    }

    pub async fn create_draft_pull_request(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<PullRequest, GitHubError> {
        rest_post(
            &self.http,
            &format!("/repos/{}/pulls", self.identity.repository),
            &self.installation_token,
            &self.api_version,
            &CreatePullRequest { title, body, head, base, draft: true },
        )
        .await
    }

    pub async fn pull_request_closing_issues(
        &self,
        number: u64,
    ) -> Result<Vec<AssociatedIssue>, GitHubError> {
        let repository = self
            .identity
            .repository
            .split_once('/')
            .ok_or_else(|| GitHubError::InvalidRepository(self.identity.repository.clone()))?;
        let mut cursor: Option<String> = None;
        let mut issues = Vec::new();
        for _ in 0..100 {
            let data: ClosingIssuesData = self
                .graphql(
                    "query($owner:String!,$name:String!,$number:Int!,$after:String){\
                       repository(owner:$owner,name:$name){pullRequest(number:$number){\
                         closingIssuesReferences(first:100,after:$after){\
                           nodes{number repository{nameWithOwner}}\
                           pageInfo{hasNextPage endCursor}}}}}",
                    &serde_json::json!({
                        "owner": repository.0,
                        "name": repository.1,
                        "number": number,
                        "after": cursor,
                    }),
                )
                .await?;
            let connection = data
                .repository
                .and_then(|repository| repository.pull_request)
                .map(|pull_request| pull_request.closing_issues)
                .ok_or(GitHubError::MissingData)?;
            issues.extend(connection.nodes.into_iter().map(|issue| AssociatedIssue {
                repository: issue.repository.name_with_owner,
                number: issue.number,
            }));
            if !connection.page_info.has_next_page {
                return Ok(issues);
            }
            cursor = connection.page_info.end_cursor;
            if cursor.is_none() {
                return Err(GitHubError::GraphQl(
                    "closingIssuesReferences hasNextPage without endCursor".into(),
                ));
            }
        }
        Err(GitHubError::GraphQl("closingIssuesReferences exceeded 10,000 entries".into()))
    }

    pub async fn app_webhook_config(&self) -> Result<AppWebhookConfig, GitHubError> {
        let jwt = app_jwt(&self.config)?;
        rest_get(&self.http, "/app/hook/config", &jwt, &self.api_version).await
    }

    pub async fn update_app_webhook(
        &self,
        url: &str,
        secret: Option<&str>,
    ) -> Result<AppWebhookConfig, GitHubError> {
        let jwt = app_jwt(&self.config)?;
        rest_patch(
            &self.http,
            "/app/hook/config",
            &jwt,
            &self.api_version,
            &AppWebhookUpdate { url, content_type: "json", insecure_ssl: "0", secret },
        )
        .await
    }

    pub async fn app_deliveries(&self) -> Result<Vec<AppDeliverySummary>, GitHubError> {
        let jwt = app_jwt(&self.config)?;
        rest_get(&self.http, "/app/hook/deliveries?per_page=100", &jwt, &self.api_version).await
    }

    pub async fn redeliver(&self, delivery_id: u64) -> Result<(), GitHubError> {
        let jwt = app_jwt(&self.config)?;
        let response = self
            .http
            .post(format!("{API_ROOT}/app/hook/deliveries/{delivery_id}/attempts"))
            .bearer_auth(jwt)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", &self.api_version)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(GitHubError::Transport)?;
        let status = response.status();
        if status == StatusCode::ACCEPTED {
            Ok(())
        } else {
            let body = response.text().await.unwrap_or_default();
            Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() })
        }
    }
}

#[derive(Serialize)]
struct AppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

fn app_jwt(config: &GitHubConfig) -> Result<String, GitHubError> {
    let pem = fs::read(&config.private_key_file)?;
    let key = EncodingKey::from_rsa_pem(&pem).map_err(GitHubError::InvalidPrivateKey)?;
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = AppClaims { iat: now - 60, exp: now + 540, iss: config.app_id.to_string() };
    jsonwebtoken::encode(&Header::new(Algorithm::RS256), &claims, &key).map_err(GitHubError::Jwt)
}

async fn rest_get<T: DeserializeOwned>(
    client: &Client,
    path: &str,
    token: &str,
    api_version: &str,
) -> Result<T, GitHubError> {
    rest_response(
        client
            .get(format!("{API_ROOT}{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", api_version)
            .send()
            .await
            .map_err(GitHubError::Transport)?,
    )
    .await
}

async fn rest_get_optional<T: DeserializeOwned>(
    client: &Client,
    path: &str,
    token: &str,
    api_version: &str,
) -> Result<Option<T>, GitHubError> {
    let response = client
        .get(format!("{API_ROOT}{path}"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", api_version)
        .send()
        .await
        .map_err(GitHubError::Transport)?;
    if response.status() == StatusCode::NOT_FOUND {
        Ok(None)
    } else {
        rest_response(response).await.map(Some)
    }
}

fn encode_path(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

async fn rest_post_empty<T: DeserializeOwned>(
    client: &Client,
    path: &str,
    token: &str,
    api_version: &str,
) -> Result<T, GitHubError> {
    rest_response(
        client
            .post(format!("{API_ROOT}{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", api_version)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(GitHubError::Transport)?,
    )
    .await
}

async fn rest_post<B: Serialize + ?Sized, T: DeserializeOwned>(
    client: &Client,
    path: &str,
    token: &str,
    api_version: &str,
    body: &B,
) -> Result<T, GitHubError> {
    rest_response(
        client
            .post(format!("{API_ROOT}{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", api_version)
            .json(body)
            .send()
            .await
            .map_err(GitHubError::Transport)?,
    )
    .await
}

async fn rest_patch<B: Serialize + ?Sized, T: DeserializeOwned>(
    client: &Client,
    path: &str,
    token: &str,
    api_version: &str,
    body: &B,
) -> Result<T, GitHubError> {
    rest_response(
        client
            .patch(format!("{API_ROOT}{path}"))
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", api_version)
            .json(body)
            .send()
            .await
            .map_err(GitHubError::Transport)?,
    )
    .await
}

async fn rest_delete(
    client: &Client,
    path: &str,
    token: &str,
    api_version: &str,
) -> Result<(), GitHubError> {
    let response = client
        .delete(format!("{API_ROOT}{path}"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", api_version)
        .send()
        .await
        .map_err(GitHubError::Transport)?;
    let status = response.status();
    if status.is_success() {
        Ok(())
    } else {
        let body = response.text().await.unwrap_or_default();
        Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() })
    }
}

async fn rest_response<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, GitHubError> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() });
    }
    response.json().await.map_err(GitHubError::Response)
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
}

#[derive(Deserialize)]
struct AppResponse {
    id: u64,
    slug: String,
}

#[derive(Deserialize)]
struct InstallationResponse {
    id: u64,
}

#[derive(Deserialize)]
struct AccessTokenResponse {
    token: String,
    expires_at: String,
    #[serde(default)]
    permissions: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct RepositoryIdentityData {
    repository: Option<RepositoryIdentityNode>,
}

#[derive(Deserialize)]
struct RepositoryIdentityNode {
    id: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize)]
struct ViewerIdentityData {
    viewer: ViewerIdentity,
}

#[derive(Deserialize)]
struct ViewerIdentity {
    id: String,
    login: String,
}

#[derive(Deserialize)]
struct PermissionResponse {
    permission: String,
    role_name: Option<String>,
}

#[derive(Serialize)]
struct ReactionRequest<'a> {
    content: &'a str,
}

#[derive(Deserialize)]
struct ReactionResponse {
    id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreatedIssueComment {
    pub id: u64,
    pub node_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IssueComment {
    pub id: u64,
    pub node_id: String,
    pub html_url: String,
    pub issue_url: String,
    pub body: Option<String>,
    pub created_at: String,
    pub user: Option<GitHubActor>,
}

impl IssueComment {
    pub fn issue_number(&self) -> Result<u64, GitHubError> {
        self.issue_url
            .rsplit_once('/')
            .and_then(|(_, number)| number.parse::<u64>().ok())
            .filter(|number| *number > 0)
            .ok_or_else(|| GitHubError::GraphQl("Issue comment has an invalid issue_url".into()))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitHubActor {
    pub login: String,
    pub node_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IssueOrPullRequest {
    pub id: u64,
    pub node_id: String,
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub state: String,
    pub pull_request: Option<serde_json::Value>,
}

impl IssueOrPullRequest {
    pub fn kind(&self) -> &'static str {
        if self.pull_request.is_some() { "pr" } else { "issue" }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepositoryDetails {
    pub default_branch: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitReference {
    #[serde(rename = "ref")]
    pub reference: String,
    pub object: GitObject,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitObject {
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GitCommit {
    pub sha: String,
    pub tree: GitObject,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PullRequest {
    pub id: u64,
    pub node_id: String,
    pub number: u64,
    pub html_url: String,
    pub draft: bool,
    pub state: String,
    pub body: Option<String>,
    pub head: PullRequestRef,
    pub base: PullRequestRef,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PullRequestRef {
    #[serde(rename = "ref")]
    pub reference: String,
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct AssociatedIssue {
    pub repository: String,
    pub number: u64,
}

#[derive(Deserialize)]
struct ClosingIssuesData {
    repository: Option<ClosingIssuesRepository>,
}

#[derive(Deserialize)]
struct ClosingIssuesRepository {
    #[serde(rename = "pullRequest")]
    pull_request: Option<ClosingIssuesPullRequest>,
}

#[derive(Deserialize)]
struct ClosingIssuesPullRequest {
    #[serde(rename = "closingIssuesReferences")]
    closing_issues: ClosingIssuesConnection,
}

#[derive(Deserialize)]
struct ClosingIssuesConnection {
    nodes: Vec<ClosingIssueNode>,
    #[serde(rename = "pageInfo")]
    page_info: ClosingIssuesPageInfo,
}

#[derive(Deserialize)]
struct ClosingIssueNode {
    number: u64,
    repository: ClosingIssueRepository,
}

#[derive(Deserialize)]
struct ClosingIssueRepository {
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Deserialize)]
struct ClosingIssuesPageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Serialize)]
struct IssueCommentRequest<'a> {
    body: &'a str,
}

#[derive(Serialize)]
struct CreateGitCommit<'a> {
    message: &'a str,
    tree: &'a str,
    parents: [&'a str; 1],
    author: GitSignature<'a>,
    committer: GitSignature<'a>,
}

#[derive(Serialize)]
struct GitSignature<'a> {
    name: &'a str,
    email: &'a str,
    date: &'a str,
}

#[derive(Serialize)]
struct CreateGitReference<'a> {
    #[serde(rename = "ref")]
    reference: &'a str,
    sha: &'a str,
}

#[derive(Serialize)]
struct UpdateGitReference<'a> {
    sha: &'a str,
    force: bool,
}

#[derive(Serialize)]
struct CreatePullRequest<'a> {
    title: &'a str,
    body: &'a str,
    head: &'a str,
    base: &'a str,
    draft: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppWebhookConfig {
    pub url: String,
    pub content_type: String,
    pub insecure_ssl: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AppDeliverySummary {
    pub id: u64,
    pub guid: String,
    pub event: String,
    pub action: Option<String>,
    pub status: String,
    pub status_code: u16,
    pub delivered_at: String,
    pub redelivery: bool,
}

#[derive(Serialize)]
struct AppWebhookUpdate<'a> {
    url: &'a str,
    content_type: &'static str,
    insecure_ssl: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret: Option<&'a str>,
}

async fn repository_identity(
    client: &Client,
    token: &str,
    repository: &RepositoryName,
) -> Result<RepositoryIdentityNode, GitHubError> {
    let response = client
        .post(GRAPHQL_URL)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .json(&serde_json::json!({
            "query": "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id nameWithOwner}}",
            "variables": {"owner": repository.owner, "name": repository.name}
        }))
        .send()
        .await
        .map_err(GitHubError::Transport)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() });
    }
    let envelope = response
        .json::<GraphQlEnvelope<RepositoryIdentityData>>()
        .await
        .map_err(GitHubError::Response)?;
    if !envelope.errors.is_empty() {
        return Err(GitHubError::GraphQl(
            envelope.errors.into_iter().map(|error| error.message).collect::<Vec<_>>().join("; "),
        ));
    }
    envelope.data.and_then(|data| data.repository).ok_or(GitHubError::MissingData)
}

async fn viewer_identity(client: &Client, token: &str) -> Result<ViewerIdentity, GitHubError> {
    let response = client
        .post(GRAPHQL_URL)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .json(&serde_json::json!({"query":"query{viewer{id login}}","variables":{}}))
        .send()
        .await
        .map_err(GitHubError::Transport)?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() });
    }
    let envelope = response
        .json::<GraphQlEnvelope<ViewerIdentityData>>()
        .await
        .map_err(GitHubError::Response)?;
    if !envelope.errors.is_empty() {
        return Err(GitHubError::GraphQl(
            envelope.errors.into_iter().map(|error| error.message).collect::<Vec<_>>().join("; "),
        ));
    }
    Ok(envelope.data.ok_or(GitHubError::MissingData)?.viewer)
}

fn bounded(value: &str, bytes: usize) -> &str {
    if value.len() <= bytes {
        return value;
    }
    let mut end = bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn validate_read_permissions(
    permissions: &BTreeMap<String, String>,
    projects_v2_enabled: bool,
) -> Result<(), GitHubError> {
    let mut missing = ["issues", "pull_requests"]
        .into_iter()
        .filter(|permission| {
            !permissions
                .get(*permission)
                .is_some_and(|level| matches!(level.as_str(), "read" | "write" | "admin"))
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if projects_v2_enabled
        && !permissions
            .get("organization_projects")
            .is_some_and(|level| matches!(level.as_str(), "read" | "write" | "admin"))
    {
        missing.push("Projects V2 (organization_projects)".into());
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(GitHubError::InsufficientPermissions(missing.join(", ")))
    }
}
