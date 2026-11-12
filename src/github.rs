use std::{collections::BTreeMap, fmt, fs, str::FromStr, sync::Arc, time::Duration};

#[path = "github/error.rs"]
mod error;
#[path = "github/graphql.rs"]
mod graphql;
#[path = "github/rate_limit.rs"]
mod rate_limit;
#[path = "github/write_rest.rs"]
mod write_rest;
pub use error::GitHubError;
use error::bounded;
pub use rate_limit::RateLimits;

use axum::http::StatusCode;
use jsonwebtoken::EncodingKey;
use octocrab::Octocrab;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::config::GitHubConfig;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RepositoryName {
    pub owner: String,
    pub name: String,
}

impl RepositoryName {
    pub(crate) fn local(name: &str) -> Self {
        Self { owner: String::new(), name: name.to_owned() }
    }
    pub fn name_with_owner(&self) -> String {
        if self.owner.is_empty() {
            self.name.clone()
        } else {
            format!("{}/{}", self.owner, self.name)
        }
    }
}

impl fmt::Display for RepositoryName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.name_with_owner())
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

#[derive(Clone)]
pub struct GitHubClient {
    app: Arc<Octocrab>,
    installation: Arc<Octocrab>,
    config: GitHubConfig,
    identity: GitHubIdentity,
    projects_v2_enabled: bool,
    rate_limits: Arc<RateLimits>,
    background: bool,
}

impl GitHubClient {
    pub async fn connect(
        config: &GitHubConfig,
        repository: &RepositoryName,
    ) -> Result<Self, GitHubError> {
        Self::connect_shared(config, repository, Arc::new(RateLimits::default())).await
    }

    pub async fn connect_shared(
        config: &GitHubConfig,
        repository: &RepositoryName,
        rate_limits: Arc<RateLimits>,
    ) -> Result<Self, GitHubError> {
        let pem = fs::read(&config.private_key_file)?;
        let key = EncodingKey::from_rsa_pem(&pem).map_err(GitHubError::InvalidPrivateKey)?;
        let app = Arc::new(
            Octocrab::builder()
                .add_retry_config(octocrab::service::middleware::retry::RetryConfig::None)
                .app(octocrab::models::AppId(config.app_id), key)
                .build()
                .map_err(|error| GitHubError::Client(error.to_string()))?,
        );
        let app_info = github_step("GET /app", app.current().app()).await?;
        let app_id = app_info.id.into_inner();
        if app_id != config.app_id {
            return Err(GitHubError::WrongApp { actual: app_id, expected: config.app_id });
        }
        let installation = github_step(
            "GET repository installation",
            app.apps().get_repository_installation(&repository.owner, &repository.name),
        )
        .await?;
        // Keep the raw id for the auto-refreshing installation client; a
        // fixed personal_token client would die permanently when the token
        // expires after one hour.
        let installation_raw_id = installation.id;
        let installation_id = installation_raw_id.into_inner();
        let access: AccessTokenResponse = github_step(
            "POST installation access token",
            app.post(&format!("/app/installations/{installation_id}/access_tokens"), None::<&()>),
        )
        .await?;
        let installation_client = Arc::new(
            app.installation(installation_raw_id)
                .map_err(|error| GitHubError::Client(error.to_string()))?,
        );
        let repository_info = github_step(
            "repository identity",
            repository_identity(&installation_client, repository, &rate_limits, installation_id),
        )
        .await?;
        let actor = github_step(
            "viewer identity",
            viewer_identity(&installation_client, &rate_limits, installation_id),
        )
        .await?;
        validate_read_permissions(&access.permissions, config.projects_v2_enabled)?;
        Ok(Self {
            app,
            rate_limits,
            background: false,
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
        if self.identity.repository == repository.name_with_owner() {
            return Ok(self.clone());
        }
        let installation = github_step(
            "GET associated repository installation",
            self.app.apps().get_repository_installation(&repository.owner, &repository.name),
        )
        .await?;
        if installation.id.into_inner() != self.identity.installation_id {
            let mut client =
                Self::connect_shared(&self.config, repository, Arc::clone(&self.rate_limits))
                    .await?;
            client.background = self.background;
            return Ok(client);
        }
        // Reuse the installation's auto-refreshing credential and admission owner.
        let mut client = self.clone();
        let data: RepositoryIdentityData = client.graphql(
            "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id nameWithOwner}}",
            &serde_json::json!({"owner":repository.owner,"name":repository.name}),
        ).await?;
        let identity = data.repository.ok_or(GitHubError::MissingData)?;
        client.identity.repository = identity.name_with_owner;
        client.identity.repository_node_id = identity.id;
        Ok(client)
    }

    pub async fn graphql<V, T>(&self, query: &str, variables: &V) -> Result<T, GitHubError>
    where
        V: Serialize + ?Sized,
        T: DeserializeOwned,
    {
        graphql::request(
            &self.installation,
            &self.rate_limits,
            self.identity.installation_id,
            self.background,
            query,
            variables,
        )
        .await
    }

    pub async fn write_retry_after(&self) -> u64 {
        self.rate_limits
            .installation(self.identity.installation_id)
            .lock()
            .await
            .retry_after(rate_limit::now())
    }

    pub async fn pull_request(&self, number: u64) -> Result<PullRequest, GitHubError> {
        self.write_rest(
            "GET",
            &format!("/repos/{}/pulls/{number}", self.identity.repository),
            None::<&()>,
        )
        .await
    }

    pub async fn reconciliation_delay(&self) -> Option<String> {
        self.rate_limits
            .installation(self.identity.installation_id)
            .lock()
            .await
            .check(rate_limit::now(), true)
            .err()
    }

    pub fn background(&self) -> Self {
        let mut client = self.clone();
        client.background = true;
        client
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
            self.write_rest("POST", &path, Some(&ReactionRequest { content })).await?;
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
        self.write_rest(
            "POST",
            &format!("/repos/{}/issues/{issue_number}/comments", self.identity.repository),
            Some(&IssueCommentRequest { body }),
        )
        .await
    }

    pub async fn issue_comments(
        &self,
        issue_number: u64,
    ) -> Result<Vec<IssueComment>, GitHubError> {
        let mut comments = Vec::new();
        for page in 1_u16..=100 {
            let page_comments: Vec<IssueComment> = self
                .write_rest(
                    "GET",
                    &format!(
                        "/repos/{}/issues/{issue_number}/comments?per_page=100&page={page}",
                        self.identity.repository
                    ),
                    None::<&()>,
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
        self.write_rest(
            "PATCH",
            &format!("/repos/{}/issues/comments/{comment_id}", self.identity.repository),
            Some(&IssueCommentRequest { body }),
        )
        .await
    }

    pub async fn issue_comment(&self, comment_id: u64) -> Result<IssueComment, GitHubError> {
        self.write_rest(
            "GET",
            &format!("/repos/{}/issues/comments/{comment_id}", self.identity.repository),
            None::<&()>,
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

    pub async fn issue_or_pull_request(
        &self,
        number: u64,
    ) -> Result<IssueOrPullRequest, GitHubError> {
        self.write_rest(
            "GET",
            &format!("/repos/{}/issues/{number}", self.identity.repository),
            None::<&()>,
        )
        .await
    }

    pub async fn repository_details(&self) -> Result<RepositoryDetails, GitHubError> {
        self.write_rest("GET", &format!("/repos/{}", self.identity.repository), None::<&()>).await
    }

    pub async fn git_reference(&self, name: &str) -> Result<Option<GitReference>, GitHubError> {
        let encoded = encode_path(name);
        match self
            .write_rest(
                "GET",
                &format!("/repos/{}/git/ref/heads/{encoded}", self.identity.repository),
                None::<&()>,
            )
            .await
        {
            Ok(reference) => Ok(Some(reference)),
            Err(GitHubError::Http { status, .. }) if status == StatusCode::NOT_FOUND => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub async fn git_commit(&self, sha: &str) -> Result<GitCommit, GitHubError> {
        self.write_rest(
            "GET",
            &format!("/repos/{}/git/commits/{sha}", self.identity.repository),
            None::<&()>,
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
        self.write_rest(
            "POST",
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
    }

    pub async fn create_git_reference(
        &self,
        name: &str,
        sha: &str,
    ) -> Result<GitReference, GitHubError> {
        let reference = format!("refs/heads/{name}");
        self.write_rest(
            "POST",
            &format!("/repos/{}/git/refs", self.identity.repository),
            Some(&CreateGitReference { reference: &reference, sha }),
        )
        .await
    }

    pub async fn update_git_reference(
        &self,
        name: &str,
        sha: &str,
    ) -> Result<GitReference, GitHubError> {
        let encoded = encode_path(name);
        self.write_rest(
            "PATCH",
            &format!("/repos/{}/git/refs/heads/{encoded}", self.identity.repository),
            Some(&UpdateGitReference { sha, force: false }),
        )
        .await
    }

    pub async fn pull_requests_for_head(
        &self,
        head: &str,
        base: &str,
    ) -> Result<Vec<PullRequest>, GitHubError> {
        let owner = self.identity.repository.split_once('/').map_or("", |(owner, _)| owner);
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("state", "all")
            .append_pair("head", &format!("{owner}:{head}"))
            .append_pair("base", base)
            .append_pair("per_page", "100")
            .finish();
        self.write_rest(
            "GET",
            &format!("/repos/{}/pulls?{query}", self.identity.repository),
            None::<&()>,
        )
        .await
    }

    pub async fn create_draft_pull_request(
        &self,
        title: &str,
        body: &str,
        head: &str,
        base: &str,
    ) -> Result<PullRequest, GitHubError> {
        self.write_rest(
            "POST",
            &format!("/repos/{}/pulls", self.identity.repository),
            Some(&CreatePullRequest { title, body, head, base, draft: true }),
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
        // The App hook deliveries endpoint is cursor-paginated and rejects
        // the offset-style `page` key with 422; request the first page only.
        let params = AppDeliveriesParams { per_page: 100 };
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

#[derive(Deserialize)]
struct AccessTokenResponse {
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
struct AppDeliveriesParams {
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
    rate_limits: &RateLimits,
    installation_id: u64,
) -> Result<RepositoryIdentityNode, GitHubError> {
    let data: RepositoryIdentityData = graphql::request(
        client, rate_limits, installation_id, false,
        "query($owner:String!,$name:String!){repository(owner:$owner,name:$name){id nameWithOwner}}",
        &serde_json::json!({"owner":repository.owner,"name":repository.name}),
    ).await?;
    data.repository.ok_or(GitHubError::MissingData)
}

async fn viewer_identity(
    client: &Octocrab,
    rate_limits: &RateLimits,
    installation_id: u64,
) -> Result<ViewerIdentity, GitHubError> {
    let data: ViewerIdentityData = graphql::request(
        client,
        rate_limits,
        installation_id,
        false,
        "query{viewer{id login}}",
        &(),
    )
    .await?;
    Ok(data.viewer)
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

const GITHUB_STEP_TIMEOUT: Duration = Duration::from_secs(30);

/// Bound one GitHub API step so an unavailable network fails with operation
/// context instead of hanging until the OS TCP timeout.
async fn github_step<T, E>(
    operation: &str,
    future: impl std::future::Future<Output = Result<T, E>>,
) -> Result<T, GitHubError>
where
    E: Into<GitHubError>,
{
    tokio::time::timeout(GITHUB_STEP_TIMEOUT, future)
        .await
        .map_err(|_| GitHubError::Timeout { operation: operation.to_owned() })?
        .map_err(Into::into)
}

#[cfg(test)]
#[path = "../tests/support/publication_github.rs"]
mod publication_github;
