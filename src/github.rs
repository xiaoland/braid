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
    #[error("credential resolved App {actual}, expected {expected}")]
    WrongApp { actual: u64, expected: u64 },
    #[error("GitHub App installation lacks required read permissions: {0}")]
    InsufficientPermissions(String),
}

impl GitHubError {
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Transport(_))
            || matches!(self, Self::Http { status, .. } if status.is_server_error())
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
