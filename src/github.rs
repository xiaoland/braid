use std::{collections::BTreeMap, fmt, fs, str::FromStr};

use axum::http::StatusCode;
use jsonwebtoken::EncodingKey;
use octocrab::Octocrab;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::config::GitHubConfig;

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
    pub fn is_unavailable(&self) -> bool {
        matches!(self, Self::Octocrab(_) | Self::ConvergencePending(_))
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
    app: Octocrab,
    installation: Octocrab,
    config: GitHubConfig,
    identity: GitHubIdentity,
    projects_v2_enabled: bool,
}

impl GitHubClient {
    pub async fn connect(
        config: &GitHubConfig,
        repository: &RepositoryName,
    ) -> Result<Self, GitHubError> {
        let pem = fs::read(&config.private_key_file)?;
        let key = EncodingKey::from_rsa_pem(&pem).map_err(GitHubError::InvalidPrivateKey)?;
        let app = Octocrab::builder()
            .app(octocrab::models::AppId(config.app_id), key)
            .build()
            .map_err(|error| GitHubError::Client(error.to_string()))?;
        let app_info = app.current().app().await?;
        let app_id = app_info.id.into_inner();
        if app_id != config.app_id {
            return Err(GitHubError::WrongApp { actual: app_id, expected: config.app_id });
        }
        let installation =
            app.apps().get_repository_installation(&repository.owner, &repository.name).await?;
        let installation_id = installation.id.into_inner();
        let access: AccessTokenResponse = app
            .post(&format!("/app/installations/{installation_id}/access_tokens"), None::<&()>)
            .await?;
        let installation_client = Octocrab::builder()
            .personal_token(access.token.clone())
            .build()
            .map_err(|error| GitHubError::Client(error.to_string()))?;
        let repository_info = repository_identity(&installation_client, repository).await?;
        let actor = viewer_identity(&installation_client).await?;
        validate_read_permissions(&access.permissions, config.projects_v2_enabled)?;
        Ok(Self {
            app,
            installation: installation_client,
            config: config.clone(),
            projects_v2_enabled: config.projects_v2_enabled,
            identity: GitHubIdentity {
                app_id,
                app_slug: app_info.slug.unwrap_or_default(),
                installation_id,
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
        let envelope: GraphQlEnvelope<T> =
            self.installation.post("/graphql", Some(&GraphQlRequest { query, variables })).await?;
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
        let permission: PermissionResponse = self.installation.get(&path, None::<&()>).await?;
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
        let reaction: ReactionResponse =
            self.installation.post(&path, Some(&ReactionRequest { content })).await?;
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
        octocrab_delete(&self.installation, &path).await
    }

    pub async fn create_issue_comment(
        &self,
        issue_number: u64,
        body: &str,
    ) -> Result<CreatedIssueComment, GitHubError> {
        self.installation
            .post(
                &format!("/repos/{}/issues/{issue_number}/comments", self.identity.repository),
                Some(&IssueCommentRequest { body }),
            )
            .await
            .map_err(GitHubError::from)
    }

    pub async fn issue_comments(
        &self,
        issue_number: u64,
    ) -> Result<Vec<IssueComment>, GitHubError> {
        let mut comments = Vec::new();
        for page in 1_u16..=100 {
            let params = PaginationParams { per_page: 100, page };
            let page_comments: Vec<IssueComment> = self
                .installation
                .get(
                    &format!("/repos/{}/issues/{issue_number}/comments", self.identity.repository),
                    Some(&params),
                )
                .await?;
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
        self.installation
            .patch(
                &format!("/repos/{}/issues/comments/{comment_id}", self.identity.repository),
                Some(&IssueCommentRequest { body }),
            )
            .await
            .map_err(GitHubError::from)
    }

    pub async fn issue_comment(&self, comment_id: u64) -> Result<IssueComment, GitHubError> {
        self.installation
            .get(
                &format!("/repos/{}/issues/comments/{comment_id}", self.identity.repository),
                None::<&()>,
            )
            .await
            .map_err(GitHubError::from)
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

    pub async fn issue_or_pull_request(
        &self,
        number: u64,
    ) -> Result<IssueOrPullRequest, GitHubError> {
        self.installation
            .get(&format!("/repos/{}/issues/{number}", self.identity.repository), None::<&()>)
            .await
            .map_err(GitHubError::from)
    }

    pub async fn repository_details(&self) -> Result<RepositoryDetails, GitHubError> {
        self.installation
            .get(&format!("/repos/{}", self.identity.repository), None::<&()>)
            .await
            .map_err(GitHubError::from)
    }

    pub async fn git_reference(&self, name: &str) -> Result<Option<GitReference>, GitHubError> {
        let encoded = encode_path(name);
        octocrab_get_optional(
            &self.installation,
            &format!("/repos/{}/git/ref/heads/{encoded}", self.identity.repository),
        )
        .await
    }

    pub async fn git_commit(&self, sha: &str) -> Result<GitCommit, GitHubError> {
        self.installation
            .get(&format!("/repos/{}/git/commits/{sha}", self.identity.repository), None::<&()>)
            .await
            .map_err(GitHubError::from)
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
        self.installation
            .post(
                &format!("/repos/{}/git/commits", self.identity.repository),
                Some(&CreateGitCommit {
                    message,
                    tree,
                    parents: [parent],
                    author: GitSignature { name: &actor, email: &email, date: authored_at },
                    committer: GitSignature { name: &actor, email: &email, date: authored_at },
                }),
            )
            .await
            .map_err(GitHubError::from)
    }

    pub async fn create_git_reference(
        &self,
        name: &str,
        sha: &str,
    ) -> Result<GitReference, GitHubError> {
        let reference = format!("refs/heads/{name}");
        self.installation
            .post(
                &format!("/repos/{}/git/refs", self.identity.repository),
                Some(&CreateGitReference { reference: &reference, sha }),
            )
            .await
            .map_err(GitHubError::from)
    }

    pub async fn update_git_reference(
        &self,
        name: &str,
        sha: &str,
    ) -> Result<GitReference, GitHubError> {
        let encoded = encode_path(name);
        self.installation
            .patch(
                &format!("/repos/{}/git/refs/heads/{encoded}", self.identity.repository),
                Some(&UpdateGitReference { sha, force: false }),
            )
            .await
            .map_err(GitHubError::from)
    }

    pub async fn open_pull_requests_for_head(
        &self,
        head: &str,
        base: &str,
    ) -> Result<Vec<PullRequest>, GitHubError> {
        let owner = self.identity.repository.split_once('/').map_or("", |(owner, _)| owner);
        let params = PullRequestsParams {
            state: "open",
            head: format!("{owner}:{head}"),
            base,
            per_page: 100,
        };
        self.installation
            .get(&format!("/repos/{}/pulls", self.identity.repository), Some(&params))
            .await
            .map_err(GitHubError::from)
    }

    pub async fn create_draft_pull_request(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<PullRequest, GitHubError> {
        self.installation
            .post(
                &format!("/repos/{}/pulls", self.identity.repository),
                Some(&CreatePullRequest { title, body, head, base, draft: true }),
            )
            .await
            .map_err(GitHubError::from)
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
        self.app.get("/app/hook/config", None::<&()>).await.map_err(GitHubError::from)
    }

    pub async fn update_app_webhook(
        &self,
        url: &str,
        secret: Option<&str>,
    ) -> Result<AppWebhookConfig, GitHubError> {
        self.app
            .patch(
                "/app/hook/config",
                Some(&AppWebhookUpdate { url, content_type: "json", insecure_ssl: "0", secret }),
            )
            .await
            .map_err(GitHubError::from)
    }

    pub async fn app_deliveries(&self) -> Result<Vec<AppDeliverySummary>, GitHubError> {
        let params = PaginationParams { per_page: 100, page: 1 };
        self.app.get("/app/hook/deliveries", Some(&params)).await.map_err(GitHubError::from)
    }

    pub async fn redeliver(&self, delivery_id: u64) -> Result<(), GitHubError> {
        let response = self
            .app
            ._post(
                format!("/app/hook/deliveries/{delivery_id}/attempts"),
                Some(&serde_json::json!({})),
            )
            .await?;
        let status = response.status();
        if status == StatusCode::ACCEPTED {
            Ok(())
        } else {
            let body = self
                .app
                .body_to_string(response)
                .await
                .map_err(|error| GitHubError::Octocrab(error.to_string()))?;
            Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() })
        }
    }
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
struct PaginationParams {
    per_page: u8,
    page: u16,
}

#[derive(Serialize)]
struct PullRequestsParams<'a> {
    state: &'a str,
    head: String,
    base: &'a str,
    per_page: u8,
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
    client: &Octocrab,
    repository: &RepositoryName,
) -> Result<RepositoryIdentityNode, GitHubError> {
    let envelope: GraphQlEnvelope<RepositoryIdentityData> = client
        .post(
            "/graphql",
            Some(&serde_json::json!({
                "query": "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id nameWithOwner}}",
                "variables": {"owner": repository.owner, "name": repository.name}
            })),
        )
        .await?;
    if !envelope.errors.is_empty() {
        return Err(GitHubError::GraphQl(
            envelope.errors.into_iter().map(|error| error.message).collect::<Vec<_>>().join("; "),
        ));
    }
    envelope.data.and_then(|data| data.repository).ok_or(GitHubError::MissingData)
}

async fn viewer_identity(client: &Octocrab) -> Result<ViewerIdentity, GitHubError> {
    let envelope: GraphQlEnvelope<ViewerIdentityData> = client
        .post(
            "/graphql",
            Some(&serde_json::json!({
                "query": "query{viewer{id login}}",
                "variables": {}
            })),
        )
        .await?;
    if !envelope.errors.is_empty() {
        return Err(GitHubError::GraphQl(
            envelope.errors.into_iter().map(|error| error.message).collect::<Vec<_>>().join("; "),
        ));
    }
    Ok(envelope.data.ok_or(GitHubError::MissingData)?.viewer)
}

async fn octocrab_get_optional<T: DeserializeOwned>(
    client: &Octocrab,
    route: impl AsRef<str>,
) -> Result<Option<T>, GitHubError> {
    match client.get::<T, _, _>(route.as_ref(), None::<&()>).await {
        Ok(value) => Ok(Some(value)),
        Err(octocrab::Error::GitHub { source, .. })
            if source.status_code == StatusCode::NOT_FOUND =>
        {
            Ok(None)
        }
        Err(error) => Err(error.into()),
    }
}

async fn octocrab_delete(client: &Octocrab, route: impl AsRef<str>) -> Result<(), GitHubError> {
    let response = client._delete(route.as_ref(), None::<&()>).await?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = client
        .body_to_string(response)
        .await
        .map_err(|error| GitHubError::Octocrab(error.to_string()))?;
    Err(GitHubError::Http { status, body: bounded(&body, 1024).to_owned() })
}

fn encode_path(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
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
