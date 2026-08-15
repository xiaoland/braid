#![allow(clippy::needless_raw_string_hashes)]

use std::{collections::BTreeSet, fmt::Write as _};

use comrak::{Arena, Options, nodes::NodeValue, parse_document};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    github::{GitHubClient, GitHubError, RepositoryName, WorkItemLocator},
    store::{
        AssociatedWorkItem, AssociationSet, CanonicalComment, CanonicalCommentSet, DeletedComment,
        StoreActor, StoreError,
    },
};

const CONTEXT_REVISION_DOMAIN: &[u8] = b"braid-context-v1\0";
const MAX_PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 100;
const MAX_MATERIALIZATION_ATTEMPTS: usize = 3;

#[derive(Debug, Error)]
pub enum ContextError {
    #[error(transparent)]
    GitHub(#[from] GitHubError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("GitHub {kind} {target} does not exist or is not visible to the Braid App")]
    Missing { kind: &'static str, target: String },
    #[error("GitHub returned an incomplete {connection} connection for {target}: {reason}")]
    Incomplete { connection: &'static str, target: String, reason: String },
    #[error("GitHub Context for {0} changed during every bounded materialization attempt")]
    Drift(String),
    #[error("GitHub Context is {bytes} bytes, above the Profile hard limit of {hard_bytes} bytes")]
    TooLarge { bytes: usize, hard_bytes: usize },
    #[error("GitHub page size must be between 1 and {MAX_PAGE_SIZE}, got {0}")]
    InvalidPageSize(usize),
}

#[derive(Debug, Clone, Serialize)]
pub struct Actor {
    pub node_id: String,
    pub login: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WorkItemReference {
    pub node_id: String,
    pub repository_node_id: String,
    pub repository: String,
    pub number: u64,
    pub kind: WorkItemKind,
    pub title: String,
    pub state: String,
    pub state_reason: Option<String>,
}

impl WorkItemReference {
    fn identity(&self) -> String {
        let noun = match self.kind {
            WorkItemKind::Issue => "Issue",
            WorkItemKind::PullRequest => "PR",
        };
        format!("GitHub {noun}: {}#{}", self.repository, self.number)
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkItemKind {
    Issue,
    PullRequest,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectEntry {
    pub title: String,
    pub fields: Vec<ProjectField>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProjectField {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommentSnapshot {
    pub node_id: String,
    pub database_id: String,
    pub repository: String,
    pub work_item_number: u64,
    pub author: Option<Actor>,
    pub created_at: String,
    pub updated_at: String,
    pub body: Option<String>,
    pub minimized: bool,
    pub minimized_reason: Option<String>,
    pub pinned: bool,
    pub deleted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct IssueSnapshot {
    pub node_id: String,
    pub database_id: String,
    pub repository_node_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub state_reason: Option<String>,
    pub updated_at: String,
    pub author: Option<Actor>,
    pub issue_type: Option<String>,
    pub assignees: Vec<Actor>,
    pub labels: Vec<String>,
    pub milestone: Option<String>,
    pub projects: Vec<ProjectEntry>,
    pub linked_branches: Vec<String>,
    pub parent: Option<WorkItemReference>,
    pub sub_issues: Vec<WorkItemReference>,
    pub blocked_by: Vec<WorkItemReference>,
    pub blocking: Vec<WorkItemReference>,
    pub duplicate_pairs: Vec<(WorkItemReference, WorkItemReference)>,
    pub associated_prs: Vec<WorkItemReference>,
    pub comments: Vec<CommentSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewSnapshot {
    pub node_id: String,
    pub database_id: String,
    pub author: Option<Actor>,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub body: String,
    pub minimized: bool,
    pub minimized_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewThreadSnapshot {
    pub node_id: String,
    pub path: String,
    pub line: Option<u64>,
    pub start_line: Option<u64>,
    pub resolved: bool,
    pub resolved_by: Option<Actor>,
    pub collapsed: bool,
    pub outdated: bool,
    pub comments: Vec<CommentSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PullRequestSnapshot {
    pub node_id: String,
    pub database_id: String,
    pub repository_node_id: String,
    pub repository: String,
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: String,
    pub draft: bool,
    pub merged: bool,
    pub updated_at: String,
    pub author: Option<Actor>,
    pub base_ref: String,
    pub head_repository: Option<String>,
    pub head_ref: String,
    pub assignees: Vec<Actor>,
    pub labels: Vec<String>,
    pub milestone: Option<String>,
    pub projects: Vec<ProjectEntry>,
    pub associated_issues: Vec<IssueSnapshot>,
    pub conversation: Vec<CommentSnapshot>,
    pub reviews: Vec<ReviewSnapshot>,
    pub review_threads: Vec<ReviewThreadSnapshot>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CanonicalContext {
    Issue(IssueSnapshot),
    PullRequest(PullRequestSnapshot),
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContextPressure {
    Normal,
    Soft,
    Hard,
}

#[derive(Debug, Clone, Serialize)]
pub struct RenderedContext {
    pub text: String,
    pub revision: String,
    pub bytes: usize,
    pub pressure: ContextPressure,
}

#[derive(Debug, Clone)]
pub struct CanonicalObservation {
    pub work_item_node_id: String,
    pub work_item_kind: &'static str,
    pub work_item_number: u64,
    pub work_item_state: String,
    pub repository_node_id: String,
    pub repository: String,
    pub object_node_id: String,
    pub database_id: String,
    pub object_kind: &'static str,
    pub version: String,
    pub digest: String,
    pub lifecycle: &'static str,
    pub author_node_id: Option<String>,
    pub author_login: Option<String>,
    pub body: Option<String>,
    pub visible_body: Option<String>,
}

pub async fn materialize_issue(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
) -> Result<IssueSnapshot, ContextError> {
    validate_page_size(page_size)?;
    for _ in 0..MAX_MATERIALIZATION_ATTEMPTS {
        let before = read_issue_marker(client, locator, page_size).await?;
        let snapshot = read_issue_snapshot(client, locator, before.clone(), page_size).await?;
        let after = read_issue_marker(client, locator, page_size).await?;
        if before == after {
            return Ok(snapshot);
        }
    }
    Err(ContextError::Drift(locator.to_string()))
}

pub async fn materialize_pull_request(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
) -> Result<PullRequestSnapshot, ContextError> {
    validate_page_size(page_size)?;
    for _ in 0..MAX_MATERIALIZATION_ATTEMPTS {
        let before = read_pr_marker(client, locator, page_size).await?;
        let snapshot = read_pr_snapshot(client, locator, before.clone(), page_size).await?;
        let after = read_pr_marker(client, locator, page_size).await?;
        if before == after {
            return Ok(snapshot);
        }
    }
    Err(ContextError::Drift(locator.to_string()))
}

fn validate_page_size(page_size: usize) -> Result<(), ContextError> {
    if (1..=MAX_PAGE_SIZE).contains(&page_size) {
        Ok(())
    } else {
        Err(ContextError::InvalidPageSize(page_size))
    }
}

#[allow(clippy::cast_precision_loss)]
pub fn render(
    context: &CanonicalContext,
    soft_ratio: f64,
    hard_bytes: usize,
) -> Result<RenderedContext, ContextError> {
    let rendered = render_complete(context, soft_ratio, hard_bytes);
    if rendered.pressure == ContextPressure::Hard {
        return Err(ContextError::TooLarge { bytes: rendered.bytes, hard_bytes });
    }
    Ok(rendered)
}

#[allow(clippy::cast_precision_loss)]
pub fn render_complete(
    context: &CanonicalContext,
    soft_ratio: f64,
    hard_bytes: usize,
) -> RenderedContext {
    let mut text = String::new();
    match context {
        CanonicalContext::Issue(issue) => render_issue(&mut text, issue, true),
        CanonicalContext::PullRequest(pull_request) => render_pull_request(&mut text, pull_request),
    }
    let bytes = text.len();
    let pressure = if bytes > hard_bytes {
        ContextPressure::Hard
    } else if bytes as f64 > hard_bytes as f64 * soft_ratio {
        ContextPressure::Soft
    } else {
        ContextPressure::Normal
    };
    let mut digest = Sha256::new();
    digest.update(CONTEXT_REVISION_DOMAIN);
    digest.update(text.as_bytes());
    RenderedContext { text, revision: hex::encode(digest.finalize()), bytes, pressure }
}

pub fn reconcile_local_state(
    context: &mut CanonicalContext,
    store: &StoreActor,
) -> Result<(), ContextError> {
    match context {
        CanonicalContext::Issue(issue) => reconcile_issue_comments(issue, store),
        CanonicalContext::PullRequest(pull_request) => {
            let work_item_digest = pull_request_root_digest(pull_request);
            for issue in &mut pull_request.associated_issues {
                reconcile_issue_comments(issue, store)?;
            }
            let excluded = store.operational_status_comment_ids(pull_request.node_id.clone())?;
            pull_request.conversation.retain(|comment| !excluded.contains(&comment.node_id));
            let mut tombstones = store.reconcile_comments(comment_set(
                &pull_request.repository_node_id,
                &pull_request.repository,
                &pull_request.node_id,
                "pr",
                pull_request.number,
                &pull_request.state,
                &pull_request.updated_at,
                &work_item_digest,
                "pr_comment",
                &pull_request.conversation,
            ))?;
            tombstones.retain(|comment| !excluded.contains(&comment.node_id));
            extend_tombstones(
                &mut pull_request.conversation,
                tombstones,
                &pull_request.repository,
                pull_request.number,
            );
            store.reconcile_associations(AssociationSet {
                anchor_node_id: pull_request.node_id.clone(),
                anchor_kind: "pr",
                observed_version: pull_request.updated_at.clone(),
                anchor_visible_description: None,
                related: pull_request
                    .associated_issues
                    .iter()
                    .map(|issue| AssociatedWorkItem {
                        node_id: issue.node_id.clone(),
                        repository_node_id: issue.repository_node_id.clone(),
                        repository: issue.repository.clone(),
                        kind: "issue",
                        number: issue.number,
                        state: issue.state.clone(),
                        visible_description: Some(filter_html_comments(&issue.body)),
                    })
                    .collect(),
            })?;
            let review_comments = pull_request
                .review_threads
                .iter()
                .flat_map(|thread| thread.comments.iter().cloned())
                .collect::<Vec<_>>();
            let tombstones = store.reconcile_comments(comment_set(
                &pull_request.repository_node_id,
                &pull_request.repository,
                &pull_request.node_id,
                "pr",
                pull_request.number,
                &pull_request.state,
                &pull_request.updated_at,
                &work_item_digest,
                "review_comment",
                &review_comments,
            ))?;
            if !tombstones.is_empty() {
                let deleted_thread = ReviewThreadSnapshot {
                    node_id: "deleted-review-comments".into(),
                    path: "unavailable after deletion".into(),
                    line: None,
                    start_line: None,
                    resolved: false,
                    resolved_by: None,
                    collapsed: false,
                    outdated: false,
                    comments: tombstones
                        .into_iter()
                        .map(|comment| {
                            deleted_comment(comment, &pull_request.repository, pull_request.number)
                        })
                        .collect(),
                };
                pull_request.review_threads.push(deleted_thread);
            }
            Ok(())
        }
    }
}

pub fn record_context_revision(
    context: &CanonicalContext,
    rendered: &RenderedContext,
    store: &StoreActor,
) -> Result<(), ContextError> {
    let node_id = match context {
        CanonicalContext::Issue(issue) => &issue.node_id,
        CanonicalContext::PullRequest(pull_request) => &pull_request.node_id,
    };
    store.set_context_revision(node_id.clone(), rendered.revision.clone())?;
    Ok(())
}

pub fn canonical_observations(context: &CanonicalContext) -> Vec<CanonicalObservation> {
    match context {
        CanonicalContext::Issue(issue) => {
            let mut observations = vec![issue_observation(issue)];
            observations.extend(
                issue
                    .comments
                    .iter()
                    .filter(|comment| !comment.deleted)
                    .map(|comment| observation(issue, "issue_comment", comment)),
            );
            observations
        }
        CanonicalContext::PullRequest(pull_request) => {
            let mut observations = vec![pull_request_observation(pull_request)];
            observations.extend(
                pull_request
                    .conversation
                    .iter()
                    .filter(|comment| !comment.deleted)
                    .map(|comment| pr_observation(pull_request, "pr_comment", comment)),
            );
            observations.extend(
                pull_request.reviews.iter().map(|review| review_observation(pull_request, review)),
            );
            observations.extend(
                pull_request
                    .review_threads
                    .iter()
                    .map(|thread| review_thread_observation(pull_request, thread)),
            );
            observations.extend(
                pull_request
                    .review_threads
                    .iter()
                    .flat_map(|thread| &thread.comments)
                    .filter(|comment| !comment.deleted)
                    .map(|comment| pr_observation(pull_request, "review_comment", comment)),
            );
            observations
        }
    }
}

fn issue_observation(issue: &IssueSnapshot) -> CanonicalObservation {
    CanonicalObservation {
        work_item_node_id: issue.node_id.clone(),
        work_item_kind: "issue",
        work_item_number: issue.number,
        work_item_state: issue.state.clone(),
        repository_node_id: issue.repository_node_id.clone(),
        repository: issue.repository.clone(),
        object_node_id: issue.node_id.clone(),
        database_id: issue.database_id.clone(),
        object_kind: "issue",
        version: issue.updated_at.clone(),
        digest: issue_root_digest(issue),
        lifecycle: "active",
        author_node_id: issue.author.as_ref().map(|author| author.node_id.clone()),
        author_login: issue.author.as_ref().map(|author| author.login.clone()),
        body: Some(issue.body.clone()),
        visible_body: Some(filter_html_comments(&issue.body)),
    }
}

fn pull_request_observation(pull_request: &PullRequestSnapshot) -> CanonicalObservation {
    CanonicalObservation {
        work_item_node_id: pull_request.node_id.clone(),
        work_item_kind: "pr",
        work_item_number: pull_request.number,
        work_item_state: pull_request.state.clone(),
        repository_node_id: pull_request.repository_node_id.clone(),
        repository: pull_request.repository.clone(),
        object_node_id: pull_request.node_id.clone(),
        database_id: pull_request.database_id.clone(),
        object_kind: "pr",
        version: pull_request.updated_at.clone(),
        digest: pull_request_root_digest(pull_request),
        lifecycle: "active",
        author_node_id: pull_request.author.as_ref().map(|author| author.node_id.clone()),
        author_login: pull_request.author.as_ref().map(|author| author.login.clone()),
        body: Some(pull_request.body.clone()),
        visible_body: Some(filter_html_comments(&pull_request.body)),
    }
}

fn review_observation(
    pull_request: &PullRequestSnapshot,
    review: &ReviewSnapshot,
) -> CanonicalObservation {
    let version = review.updated_at.clone();
    let lifecycle = if review.minimized {
        "minimized"
    } else if review.state.eq_ignore_ascii_case("dismissed") {
        "dismissed"
    } else {
        "active"
    };
    CanonicalObservation {
        work_item_node_id: pull_request.node_id.clone(),
        work_item_kind: "pr",
        work_item_number: pull_request.number,
        work_item_state: pull_request.state.clone(),
        repository_node_id: pull_request.repository_node_id.clone(),
        repository: pull_request.repository.clone(),
        object_node_id: review.node_id.clone(),
        database_id: review.database_id.clone(),
        object_kind: "review",
        digest: object_digest(&review.node_id, &version, Some(&review.body)),
        version,
        lifecycle,
        author_node_id: review.author.as_ref().map(|author| author.node_id.clone()),
        author_login: review.author.as_ref().map(|author| author.login.clone()),
        body: Some(review.body.clone()),
        visible_body: Some(filter_html_comments(&review.body)),
    }
}

fn review_thread_observation(
    pull_request: &PullRequestSnapshot,
    thread: &ReviewThreadSnapshot,
) -> CanonicalObservation {
    let lifecycle = if thread.resolved { "resolved" } else { "active" };
    let version = format!(
        "{}:{}:{}:{}:{}:{}",
        lifecycle,
        thread.collapsed,
        thread.outdated,
        thread.path,
        thread.line.map_or_else(String::new, |line| line.to_string()),
        thread.comments.iter().map(|comment| comment.updated_at.as_str()).max().unwrap_or("")
    );
    CanonicalObservation {
        work_item_node_id: pull_request.node_id.clone(),
        work_item_kind: "pr",
        work_item_number: pull_request.number,
        work_item_state: pull_request.state.clone(),
        repository_node_id: pull_request.repository_node_id.clone(),
        repository: pull_request.repository.clone(),
        object_node_id: thread.node_id.clone(),
        database_id: String::new(),
        object_kind: "review_thread",
        digest: object_digest(&thread.node_id, &version, None),
        version,
        lifecycle,
        author_node_id: thread.resolved_by.as_ref().map(|author| author.node_id.clone()),
        author_login: thread.resolved_by.as_ref().map(|author| author.login.clone()),
        body: None,
        visible_body: None,
    }
}

fn observation(
    issue: &IssueSnapshot,
    object_kind: &'static str,
    comment: &CommentSnapshot,
) -> CanonicalObservation {
    let (version, digest, lifecycle) = comment_identity(comment);
    CanonicalObservation {
        work_item_node_id: issue.node_id.clone(),
        work_item_kind: "issue",
        work_item_number: issue.number,
        work_item_state: issue.state.clone(),
        repository_node_id: issue.repository_node_id.clone(),
        repository: issue.repository.clone(),
        object_node_id: comment.node_id.clone(),
        database_id: comment.database_id.clone(),
        object_kind,
        version,
        digest,
        lifecycle,
        author_node_id: comment.author.as_ref().map(|author| author.node_id.clone()),
        author_login: comment.author.as_ref().map(|author| author.login.clone()),
        body: comment.body.clone(),
        visible_body: comment.body.as_deref().map(filter_html_comments),
    }
}

fn pr_observation(
    pull_request: &PullRequestSnapshot,
    object_kind: &'static str,
    comment: &CommentSnapshot,
) -> CanonicalObservation {
    let (version, digest, lifecycle) = comment_identity(comment);
    CanonicalObservation {
        work_item_node_id: pull_request.node_id.clone(),
        work_item_kind: "pr",
        work_item_number: pull_request.number,
        work_item_state: pull_request.state.clone(),
        repository_node_id: pull_request.repository_node_id.clone(),
        repository: pull_request.repository.clone(),
        object_node_id: comment.node_id.clone(),
        database_id: comment.database_id.clone(),
        object_kind,
        version,
        digest,
        lifecycle,
        author_node_id: comment.author.as_ref().map(|author| author.node_id.clone()),
        author_login: comment.author.as_ref().map(|author| author.login.clone()),
        body: comment.body.clone(),
        visible_body: comment.body.as_deref().map(filter_html_comments),
    }
}

fn comment_identity(comment: &CommentSnapshot) -> (String, String, &'static str) {
    let lifecycle = if comment.minimized { "minimized" } else { "active" };
    let version = format!(
        "{}:{}:{}",
        comment.updated_at,
        lifecycle,
        comment.minimized_reason.as_deref().unwrap_or_default()
    );
    let digest = object_digest(&comment.node_id, &version, comment.body.as_deref());
    (version, digest, lifecycle)
}

fn object_digest(node_id: &str, version: &str, body: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(node_id.as_bytes());
    digest.update(b"\0");
    digest.update(version.as_bytes());
    digest.update(b"\0");
    if let Some(body) = body {
        digest.update(body.as_bytes());
    }
    hex::encode(digest.finalize())
}

fn reconcile_issue_comments(
    issue: &mut IssueSnapshot,
    store: &StoreActor,
) -> Result<(), ContextError> {
    let work_item_digest = issue_root_digest(issue);
    let excluded = store.operational_status_comment_ids(issue.node_id.clone())?;
    issue.comments.retain(|comment| !excluded.contains(&comment.node_id));
    let mut tombstones = store.reconcile_comments(comment_set(
        &issue.repository_node_id,
        &issue.repository,
        &issue.node_id,
        "issue",
        issue.number,
        &issue.state,
        &issue.updated_at,
        &work_item_digest,
        "issue_comment",
        &issue.comments,
    ))?;
    tombstones.retain(|comment| !excluded.contains(&comment.node_id));
    extend_tombstones(&mut issue.comments, tombstones, &issue.repository, issue.number);
    store.reconcile_associations(AssociationSet {
        anchor_node_id: issue.node_id.clone(),
        anchor_kind: "issue",
        observed_version: issue.updated_at.clone(),
        anchor_visible_description: Some(filter_html_comments(&issue.body)),
        related: issue
            .associated_prs
            .iter()
            .map(|pull_request| AssociatedWorkItem {
                node_id: pull_request.node_id.clone(),
                repository_node_id: pull_request.repository_node_id.clone(),
                repository: pull_request.repository.clone(),
                kind: "pr",
                number: pull_request.number,
                state: pull_request.state.clone(),
                visible_description: None,
            })
            .collect(),
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn comment_set(
    repository_node_id: &str,
    repository: &str,
    work_item_node_id: &str,
    work_item_kind: &'static str,
    work_item_number: u64,
    work_item_state: &str,
    work_item_version: &str,
    work_item_digest: &str,
    object_kind: &'static str,
    comments: &[CommentSnapshot],
) -> CanonicalCommentSet {
    CanonicalCommentSet {
        repository_node_id: repository_node_id.to_owned(),
        repository: repository.to_owned(),
        work_item_node_id: work_item_node_id.to_owned(),
        work_item_kind,
        work_item_number,
        work_item_state: work_item_state.to_owned(),
        work_item_version: work_item_version.to_owned(),
        work_item_digest: work_item_digest.to_owned(),
        object_kind,
        comments: comments
            .iter()
            .filter(|comment| !comment.deleted)
            .map(|comment| {
                let (version, digest, lifecycle) = comment_identity(comment);
                CanonicalComment {
                    node_id: comment.node_id.clone(),
                    database_id: comment.database_id.clone(),
                    object_kind,
                    version,
                    digest,
                    lifecycle,
                    author_node_id: comment.author.as_ref().map(|author| author.node_id.clone()),
                    author_login: comment.author.as_ref().map(|author| author.login.clone()),
                    created_at: comment.created_at.clone(),
                    updated_at: comment.updated_at.clone(),
                    pinned: comment.pinned,
                }
            })
            .collect(),
    }
}

fn root_projection_digest(node_id: &str, projection: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"braid-root-projection-v1\0");
    digest.update(node_id.as_bytes());
    digest.update(b"\0");
    digest.update(projection.as_bytes());
    hex::encode(digest.finalize())
}

fn issue_root_digest(issue: &IssueSnapshot) -> String {
    let mut root = issue.clone();
    root.comments.clear();
    let mut projection = String::new();
    render_issue(&mut projection, &root, true);
    root_projection_digest(&root.node_id, &projection)
}

fn pull_request_root_digest(pull_request: &PullRequestSnapshot) -> String {
    let mut root = pull_request.clone();
    root.associated_issues.clear();
    root.conversation.clear();
    root.reviews.clear();
    root.review_threads.clear();
    let mut projection = String::new();
    render_pull_request(&mut projection, &root);
    root_projection_digest(&root.node_id, &projection)
}

fn extend_tombstones(
    comments: &mut Vec<CommentSnapshot>,
    tombstones: Vec<DeletedComment>,
    repository: &str,
    work_item_number: u64,
) {
    comments.extend(
        tombstones
            .into_iter()
            .map(|comment| deleted_comment(comment, repository, work_item_number)),
    );
    comments.sort_by(|left, right| {
        left.created_at.cmp(&right.created_at).then(left.node_id.cmp(&right.node_id))
    });
}

fn deleted_comment(
    comment: DeletedComment,
    repository: &str,
    work_item_number: u64,
) -> CommentSnapshot {
    CommentSnapshot {
        node_id: comment.node_id,
        database_id: comment.database_id,
        repository: repository.to_owned(),
        work_item_number,
        author: comment
            .author_login
            .map(|login| Actor { node_id: comment.author_node_id.unwrap_or_default(), login }),
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        body: None,
        minimized: false,
        minimized_reason: None,
        pinned: comment.pinned,
        deleted: true,
    }
}

fn render_issue(output: &mut String, issue: &IssueSnapshot, complete: bool) {
    push_line(output, &format!("# GitHub Issue: {}#{}", issue.repository, issue.number));
    push_line(output, &issue.title);
    output.push('\n');
    push_state(output, &issue.state, issue.state_reason.as_deref());
    if !complete {
        render_issue_relationships(output, issue);
        return;
    }
    push_actor(output, "Author", issue.author.as_ref());
    if let Some(issue_type) = &issue.issue_type {
        push_line(output, &format!("Type: {issue_type}"));
    }
    push_actor_set(output, "Assignees", &issue.assignees);
    push_string_set(output, "Labels", &issue.labels);
    if let Some(milestone) = &issue.milestone {
        push_line(output, &format!("Milestone: {milestone}"));
    }
    render_projects(output, &issue.projects);
    push_string_set(output, "Development branches", &issue.linked_branches);
    render_issue_relationships(output, issue);
    if !issue.body.is_empty() {
        push_section(output, "Description");
        push_body(output, &filter_html_comments(&issue.body));
    }
    if !issue.comments.is_empty() {
        push_section(output, "Comments");
        for comment in &issue.comments {
            render_comment(output, comment, "Comment");
        }
    }
}

fn render_pull_request(output: &mut String, pull_request: &PullRequestSnapshot) {
    for issue in &pull_request.associated_issues {
        let complete = !issue.state.eq_ignore_ascii_case("closed");
        render_issue(output, issue, complete);
        output.push_str("\n---\n\n");
    }
    push_line(output, &format!("# GitHub PR: {}#{}", pull_request.repository, pull_request.number));
    push_line(output, &pull_request.title);
    output.push('\n');
    push_line(output, &format!("State: {}", pull_request.state.to_ascii_lowercase()));
    let readiness = if pull_request.merged {
        "merged"
    } else if pull_request.draft {
        "draft"
    } else {
        "ready"
    };
    push_line(output, &format!("Lifecycle: {readiness}"));
    push_actor(output, "Author", pull_request.author.as_ref());
    push_line(output, &format!("Base: {}", pull_request.base_ref));
    let head = pull_request.head_repository.as_ref().map_or_else(
        || pull_request.head_ref.clone(),
        |repository| format!("{repository}:{}", pull_request.head_ref),
    );
    push_line(output, &format!("Head: {head}"));
    push_actor_set(output, "Assignees", &pull_request.assignees);
    push_string_set(output, "Labels", &pull_request.labels);
    if let Some(milestone) = &pull_request.milestone {
        push_line(output, &format!("Milestone: {milestone}"));
    }
    render_projects(output, &pull_request.projects);
    if !pull_request.body.is_empty() {
        push_section(output, "Description");
        push_body(output, &filter_html_comments(&pull_request.body));
    }
    if !pull_request.conversation.is_empty() {
        push_section(output, "Conversation");
        for comment in &pull_request.conversation {
            render_comment(output, comment, "Comment");
        }
    }
    if !pull_request.reviews.is_empty() {
        push_section(output, "Reviews");
        for review in &pull_request.reviews {
            let author = review.author.as_ref().map_or("@ghost", |actor| actor.login.as_str());
            push_line(output, &format!("### Review: {} by @{author}", review.database_id));
            push_line(output, &format!("State: {}", review.state.to_ascii_lowercase()));
            push_line(output, &format!("Posted: {}", review.created_at));
            if review.updated_at != review.created_at {
                push_line(output, &format!("Updated: {}", review.updated_at));
            }
            if review.minimized {
                let reason = review.minimized_reason.as_deref().unwrap_or("unspecified");
                push_line(output, &format!("State: minimized ({reason})"));
            } else if !review.body.is_empty() {
                output.push('\n');
                push_body(output, &filter_html_comments(&review.body));
            }
        }
    }
    if !pull_request.review_threads.is_empty() {
        push_section(output, "Review Threads");
        for thread in &pull_request.review_threads {
            let mut location = thread.path.clone();
            if let Some(line) = thread.line {
                let _ = write!(location, ":{line}");
            }
            push_line(output, &format!("### Review thread at {location}"));
            push_line(output, &format!("Location: {location}"));
            let mut states = Vec::new();
            if thread.resolved {
                states.push("resolved");
            }
            if thread.collapsed {
                states.push("collapsed");
            }
            if thread.outdated {
                states.push("outdated");
            }
            if states.is_empty() {
                states.push("open");
            }
            push_line(output, &format!("State: {}", states.join(", ")));
            if thread.resolved {
                push_actor(output, "Resolved by", thread.resolved_by.as_ref());
            }
            if thread.resolved || thread.collapsed {
                render_thread_metadata(output, &thread.comments);
            } else {
                for comment in &thread.comments {
                    render_comment(output, comment, "Review comment");
                }
            }
        }
    }
}

fn render_issue_relationships(output: &mut String, issue: &IssueSnapshot) {
    if let Some(parent) = &issue.parent {
        push_line(output, &format!("Parent: {}", parent.identity()));
    }
    push_references(output, "Sub-issues", &issue.sub_issues);
    push_references(output, "Blocked by", &issue.blocked_by);
    push_references(output, "Blocking", &issue.blocking);
    push_references(output, "Associated PRs", &issue.associated_prs);
    if !issue.duplicate_pairs.is_empty() {
        let values = issue
            .duplicate_pairs
            .iter()
            .map(|(duplicate, canonical)| {
                format!("{} → {}", duplicate.identity(), canonical.identity())
            })
            .collect::<Vec<_>>();
        push_line(output, &format!("Duplicates: {}", values.join(", ")));
    }
}

fn render_projects(output: &mut String, projects: &[ProjectEntry]) {
    for project in projects {
        let fields = project
            .fields
            .iter()
            .map(|field| format!("{}={}", field.name, field.value))
            .collect::<Vec<_>>();
        if fields.is_empty() {
            push_line(output, &format!("Project: {}", project.title));
        } else {
            push_line(output, &format!("Project: {} ({})", project.title, fields.join(", ")));
        }
    }
}

fn render_comment(output: &mut String, comment: &CommentSnapshot, noun: &str) {
    let author = comment.author.as_ref().map_or("ghost", |actor| actor.login.as_str());
    let reference_kind =
        if noun == "Review comment" { "pullrequestreviewcomment" } else { "issuecomment" };
    push_line(
        output,
        &format!(
            "### {noun}: {}#{reference_kind}-{} by @{author}",
            comment.repository, comment.database_id,
        ),
    );
    push_line(output, &format!("Posted: {}", comment.created_at));
    if comment.updated_at != comment.created_at {
        push_line(output, &format!("Updated: {}", comment.updated_at));
    }
    if comment.deleted {
        push_line(output, "State: deleted");
        output.push('\n');
        return;
    }
    if comment.minimized {
        let reason = comment.minimized_reason.as_deref().unwrap_or("unspecified");
        push_line(output, &format!("State: minimized ({reason})"));
        output.push('\n');
        return;
    }
    if comment.pinned {
        push_line(output, "Pinned: yes");
    }
    if let Some(body) = &comment.body {
        output.push('\n');
        push_body(output, &filter_html_comments(body));
    }
}

fn render_thread_metadata(output: &mut String, comments: &[CommentSnapshot]) {
    let authors = comments
        .iter()
        .filter_map(|comment| comment.author.as_ref().map(|actor| format!("@{}", actor.login)))
        .collect::<BTreeSet<_>>();
    if !authors.is_empty() {
        push_line(
            output,
            &format!("Authors: {}", authors.into_iter().collect::<Vec<_>>().join(", ")),
        );
    }
    if let Some(first) = comments.first() {
        push_line(output, &format!("Posted: {}", first.created_at));
    }
    if let Some(last) = comments.last() {
        push_line(output, &format!("Updated: {}", last.updated_at));
    }
    output.push('\n');
}

fn push_state(output: &mut String, state: &str, reason: Option<&str>) {
    let state = state.to_ascii_lowercase();
    match reason {
        Some(reason) => {
            push_line(output, &format!("State: {state} ({})", reason.to_ascii_lowercase()));
        }
        None => push_line(output, &format!("State: {state}")),
    }
}

fn push_actor(output: &mut String, label: &str, actor: Option<&Actor>) {
    if let Some(actor) = actor {
        push_line(output, &format!("{label}: @{}", actor.login));
    }
}

fn push_actor_set(output: &mut String, label: &str, actors: &[Actor]) {
    let values = actors.iter().map(|actor| format!("@{}", actor.login)).collect::<Vec<_>>();
    if !values.is_empty() {
        push_line(output, &format!("{label}: {}", values.join(", ")));
    }
}

fn push_string_set(output: &mut String, label: &str, values: &[String]) {
    if !values.is_empty() {
        push_line(output, &format!("{label}: {}", values.join(", ")));
    }
}

fn push_references(output: &mut String, label: &str, references: &[WorkItemReference]) {
    if !references.is_empty() {
        push_line(
            output,
            &format!(
                "{label}: {}",
                references.iter().map(WorkItemReference::identity).collect::<Vec<_>>().join(", ")
            ),
        );
    }
}

fn push_section(output: &mut String, title: &str) {
    output.push('\n');
    push_line(output, &format!("## {title}"));
    output.push('\n');
}

fn push_body(output: &mut String, body: &str) {
    output.push_str(body.trim_matches('\n'));
    output.push_str("\n\n");
}

fn push_line(output: &mut String, line: &str) {
    output.push_str(line);
    output.push('\n');
}

pub fn filter_html_comments(markdown: &str) -> String {
    let arena = Arena::new();
    let root = parse_document(&arena, markdown, &Options::default());
    let line_offsets = line_offsets(markdown);
    let mut ranges = Vec::new();
    for node in root.descendants() {
        let data = node.data.borrow();
        let is_comment = match &data.value {
            NodeValue::HtmlInline(literal) => literal.trim_start().starts_with("<!--"),
            NodeValue::HtmlBlock(block) => block.literal.trim_start().starts_with("<!--"),
            _ => false,
        };
        if is_comment && let Some(range) = source_range(markdown, &line_offsets, data.sourcepos) {
            ranges.push(range);
        }
    }
    if ranges.is_empty() {
        return markdown.to_owned();
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        if let Some(last) = merged.last_mut()
            && start <= last.1
        {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    let mut visible = String::with_capacity(markdown.len());
    let mut cursor = 0;
    for (start, end) in merged {
        visible.push_str(&markdown[cursor..start]);
        cursor = end;
    }
    visible.push_str(&markdown[cursor..]);
    visible
}

fn line_offsets(markdown: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    for (offset, byte) in markdown.bytes().enumerate() {
        if byte == b'\n' {
            offsets.push(offset + 1);
        }
    }
    offsets
}

fn source_range(
    markdown: &str,
    line_offsets: &[usize],
    source: comrak::nodes::Sourcepos,
) -> Option<(usize, usize)> {
    let start_line = *line_offsets.get(source.start.line.checked_sub(1)?)?;
    let end_line = *line_offsets.get(source.end.line.checked_sub(1)?)?;
    let start = start_line.checked_add(source.start.column.checked_sub(1)?)?;
    let end = end_line.checked_add(source.end.column)?;
    (start <= end && end <= markdown.len()).then_some((start, end))
}

// GitHub materialization queries and response adapters follow. They are kept in
// this module because partial canonical data must never escape into projection.

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ActorRaw {
    id: String,
    login: String,
}

impl From<ActorRaw> for Actor {
    fn from(actor: ActorRaw) -> Self {
        Self { node_id: actor.id, login: actor.login }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RepositoryRaw {
    id: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct MilestoneRaw {
    title: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct IssueTypeRaw {
    name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct IssueCoreRaw {
    id: String,
    #[serde(rename = "fullDatabaseId")]
    full_database_id: String,
    number: u64,
    title: String,
    body: String,
    state: String,
    #[serde(rename = "stateReason")]
    state_reason: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    author: Option<ActorRaw>,
    #[serde(rename = "issueType")]
    issue_type: Option<IssueTypeRaw>,
    milestone: Option<MilestoneRaw>,
    parent: Option<ReferenceRaw>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct PullRequestCoreRaw {
    id: String,
    #[serde(rename = "fullDatabaseId")]
    full_database_id: String,
    number: u64,
    title: String,
    body: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    merged: bool,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    author: Option<ActorRaw>,
    #[serde(rename = "baseRefName")]
    base_ref_name: String,
    #[serde(rename = "headRefName")]
    head_ref_name: String,
    #[serde(rename = "headRepository")]
    head_repository: Option<NameRepositoryRaw>,
    milestone: Option<MilestoneRaw>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct NameRepositoryRaw {
    id: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ReferenceRaw {
    id: String,
    number: u64,
    title: String,
    state: String,
    #[serde(rename = "stateReason")]
    state_reason: Option<String>,
    repository: NameRepositoryRaw,
}

impl ReferenceRaw {
    fn issue_reference(self) -> WorkItemReference {
        WorkItemReference {
            node_id: self.id,
            repository_node_id: self.repository.id,
            repository: self.repository.name_with_owner,
            number: self.number,
            kind: WorkItemKind::Issue,
            title: self.title,
            state: self.state,
            state_reason: self.state_reason,
        }
    }

    fn pr_reference(self) -> WorkItemReference {
        WorkItemReference {
            node_id: self.id,
            repository_node_id: self.repository.id,
            repository: self.repository.name_with_owner,
            number: self.number,
            kind: WorkItemKind::PullRequest,
            title: self.title,
            state: self.state,
            state_reason: None,
        }
    }
}

#[derive(Debug, Deserialize)]
struct IssueRootData {
    repository: Option<IssueRootRepository>,
}

#[derive(Debug, Deserialize)]
struct IssueRootRepository {
    id: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    issue: Option<IssueCoreRaw>,
}

#[derive(Debug, Deserialize)]
struct PullRequestRootData {
    repository: Option<PullRequestRootRepository>,
}

#[derive(Debug, Deserialize)]
struct PullRequestRootRepository {
    id: String,
    #[serde(rename = "nameWithOwner")]
    name_with_owner: String,
    #[serde(rename = "pullRequest")]
    pull_request: Option<PullRequestCoreRaw>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Connection<T> {
    nodes: Vec<T>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
struct IssueConnectionData<T> {
    repository: Option<IssueConnectionRepository<T>>,
}

#[derive(Debug, Deserialize)]
struct IssueConnectionRepository<T> {
    issue: Option<IssueConnectionRoot<T>>,
}

#[derive(Debug, Deserialize)]
struct IssueConnectionRoot<T> {
    items: Connection<T>,
}

#[derive(Debug, Deserialize)]
struct PullRequestConnectionData<T> {
    repository: Option<PullRequestConnectionRepository<T>>,
}

#[derive(Debug, Deserialize)]
struct PullRequestConnectionRepository<T> {
    #[serde(rename = "pullRequest")]
    pull_request: Option<PullRequestConnectionRoot<T>>,
}

#[derive(Debug, Deserialize)]
struct PullRequestConnectionRoot<T> {
    items: Connection<T>,
}

#[derive(Debug, Serialize)]
struct ConnectionVariables<'a> {
    owner: &'a str,
    name: &'a str,
    number: u64,
    cursor: Option<&'a str>,
    #[serde(rename = "pageSize")]
    page_size: usize,
}

const ISSUE_CORE_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!){
  repository(owner:$owner,name:$name){
    id nameWithOwner
    issue(number:$number){
      id fullDatabaseId number title body state stateReason updatedAt
      author{login ... on Bot{id} ... on EnterpriseUserAccount{id} ... on Mannequin{id} ... on Organization{id} ... on User{id}}
      issueType{name}
      milestone{title}
      parent{id number title state stateReason repository{id nameWithOwner}}
    }
  }
}"#;

const PR_CORE_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!){
  repository(owner:$owner,name:$name){
    id nameWithOwner
    pullRequest(number:$number){
      id fullDatabaseId number title body state isDraft merged updatedAt
      author{login ... on Bot{id} ... on EnterpriseUserAccount{id} ... on Mannequin{id} ... on Organization{id} ... on User{id}}
      baseRefName headRefName headRepository{id nameWithOwner}
      milestone{title}
    }
  }
}"#;

const ISSUE_ASSIGNEES_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:assignees(first:$pageSize,after:$cursor){nodes{id login} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_LABELS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:labels(first:$pageSize,after:$cursor){nodes{name} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_PROJECTS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:projectItems(first:$pageSize,after:$cursor){nodes{id project{title}} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const PR_PROJECTS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:projectItems(first:$pageSize,after:$cursor){nodes{id project{title}} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const PROJECT_FIELD_VALUES_QUERY: &str = r#"
query($id:ID!,$cursor:String,$pageSize:Int!){
  node(id:$id){... on ProjectV2Item{
    items:fieldValues(first:$pageSize,after:$cursor){
      nodes{
        __typename
        ... on ProjectV2ItemFieldTextValue{field{... on ProjectV2FieldCommon{name}} text}
        ... on ProjectV2ItemFieldDateValue{field{... on ProjectV2FieldCommon{name}} date}
        ... on ProjectV2ItemFieldSingleSelectValue{field{... on ProjectV2FieldCommon{name}} name}
        ... on ProjectV2ItemFieldNumberValue{field{... on ProjectV2FieldCommon{name}} number}
        ... on ProjectV2ItemFieldIterationValue{field{... on ProjectV2FieldCommon{name}} title startDate duration}
        ... on ProjectV2ItemFieldMultiSelectValue{field{... on ProjectV2FieldCommon{name}} value}
        ... on ProjectV2ItemFieldLabelValue{field{... on ProjectV2FieldCommon{name}} labels(first:100){nodes{name} pageInfo{hasNextPage endCursor}}}
        ... on ProjectV2ItemFieldMilestoneValue{field{... on ProjectV2FieldCommon{name}} milestone{title}}
        ... on ProjectV2ItemFieldPullRequestValue{field{... on ProjectV2FieldCommon{name}} pullRequests(first:100){nodes{number repository{id nameWithOwner}} pageInfo{hasNextPage endCursor}}}
        ... on ProjectV2ItemFieldRepositoryValue{field{... on ProjectV2FieldCommon{name}} repository{id nameWithOwner}}
        ... on ProjectV2ItemFieldReviewerValue{field{... on ProjectV2FieldCommon{name}} reviewers(first:100){nodes{__typename ... on Bot{login} ... on EnterpriseTeam{name} ... on Mannequin{login} ... on Team{name} ... on User{login}} pageInfo{hasNextPage endCursor}}}
        ... on ProjectV2ItemFieldUserValue{field{... on ProjectV2FieldCommon{name}} users(first:100){nodes{login} pageInfo{hasNextPage endCursor}}}
        ... on ProjectV2ItemIssueFieldValue{
          field{... on ProjectV2FieldCommon{name}}
          issueFieldValue{
            __typename
            ... on IssueFieldDateValue{value}
            ... on IssueFieldTextValue{value}
            ... on IssueFieldNumberValue{value}
            ... on IssueFieldSingleSelectValue{name}
            ... on IssueFieldMultiSelectValue{value}
          }
        }
      }
      pageInfo{hasNextPage endCursor}
    }
  }}
}"#;

const ISSUE_LINKED_BRANCHES_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:linkedBranches(first:$pageSize,after:$cursor){nodes{id ref{name repository{id nameWithOwner}}} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_SUB_ISSUES_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:subIssues(first:$pageSize,after:$cursor){nodes{id number title state stateReason repository{id nameWithOwner}} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_BLOCKED_BY_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:blockedBy(first:$pageSize,after:$cursor){nodes{id number title state stateReason repository{id nameWithOwner}} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_BLOCKING_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:blocking(first:$pageSize,after:$cursor){nodes{id number title state stateReason repository{id nameWithOwner}} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_ASSOCIATED_PRS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:closedByPullRequestsReferences(first:$pageSize,after:$cursor,includeClosedPrs:true){
      nodes{id number title state repository{id nameWithOwner}}
      pageInfo{hasNextPage endCursor}
    }
  }}
}"#;

const ISSUE_COMMENTS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:comments(first:$pageSize,after:$cursor){nodes{
      id fullDatabaseId author{login ... on Bot{id} ... on EnterpriseUserAccount{id} ... on Mannequin{id} ... on Organization{id} ... on User{id}} createdAt updatedAt lastEditedAt
      body isMinimized minimizedReason isPinned
    } pageInfo{hasNextPage endCursor}}
  }}
}"#;

const ISSUE_DUPLICATES_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){issue(number:$number){
    items:timelineItems(first:$pageSize,after:$cursor,itemTypes:[MARKED_AS_DUPLICATE_EVENT]){
      nodes{... on MarkedAsDuplicateEvent{
        id
        duplicate{__typename ... on Issue{id number title state stateReason repository{id nameWithOwner}} ... on PullRequest{id number title state repository{id nameWithOwner}}}
        canonical{__typename ... on Issue{id number title state stateReason repository{id nameWithOwner}} ... on PullRequest{id number title state repository{id nameWithOwner}}}
      }}
      pageInfo{hasNextPage endCursor}
    }
  }}
}"#;

const PR_ASSIGNEES_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:assignees(first:$pageSize,after:$cursor){nodes{id login} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const PR_LABELS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:labels(first:$pageSize,after:$cursor){nodes{name} pageInfo{hasNextPage endCursor}}
  }}
}"#;

const PR_ASSOCIATED_ISSUES_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:closingIssuesReferences(first:$pageSize,after:$cursor){
      nodes{id number title state stateReason repository{id nameWithOwner}}
      pageInfo{hasNextPage endCursor}
    }
  }}
}"#;

const PR_COMMENTS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:comments(first:$pageSize,after:$cursor){nodes{
      id fullDatabaseId author{login ... on Bot{id} ... on EnterpriseUserAccount{id} ... on Mannequin{id} ... on Organization{id} ... on User{id}} createdAt updatedAt lastEditedAt
      body isMinimized minimizedReason isPinned
    } pageInfo{hasNextPage endCursor}}
  }}
}"#;

const PR_REVIEWS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:reviews(first:$pageSize,after:$cursor){nodes{
      id fullDatabaseId author{login ... on Bot{id} ... on EnterpriseUserAccount{id} ... on Mannequin{id} ... on Organization{id} ... on User{id}} state createdAt submittedAt updatedAt
      body isMinimized minimizedReason
    } pageInfo{hasNextPage endCursor}}
  }}
}"#;

const PR_REVIEW_THREADS_QUERY: &str = r#"
query($owner:String!,$name:String!,$number:Int!,$cursor:String,$pageSize:Int!){
  repository(owner:$owner,name:$name){pullRequest(number:$number){
    items:reviewThreads(first:$pageSize,after:$cursor){nodes{
      id path line startLine isResolved isCollapsed isOutdated
      resolvedBy{id login}
    } pageInfo{hasNextPage endCursor}}
  }}
}"#;

const REVIEW_THREAD_COMMENTS_QUERY: &str = r#"
query($id:ID!,$cursor:String,$pageSize:Int!){
  node(id:$id){... on PullRequestReviewThread{
    items:comments(first:$pageSize,after:$cursor){nodes{
      id fullDatabaseId author{login ... on Bot{id} ... on EnterpriseUserAccount{id} ... on Mannequin{id} ... on Organization{id} ... on User{id}} createdAt updatedAt lastEditedAt
      body isMinimized minimizedReason
    } pageInfo{hasNextPage endCursor}}
  }}
}"#;

#[derive(Debug, Clone, Deserialize)]
struct LabelRaw {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ProjectRaw {
    id: String,
    project: ProjectTitleRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct ProjectTitleRaw {
    title: String,
}

#[derive(Debug, Clone, Deserialize)]
struct FieldRaw {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NamedRaw {
    name: String,
}

#[derive(Debug, Clone, Deserialize)]
struct LoginRaw {
    login: String,
}

#[derive(Debug, Clone, Deserialize)]
struct NumberReferenceRaw {
    number: u64,
    repository: NameRepositoryRaw,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
enum ReviewerRaw {
    Bot { login: String },
    EnterpriseTeam { name: String },
    Mannequin { login: String },
    Team { name: String },
    User { login: String },
}

impl ReviewerRaw {
    fn display_name(self) -> String {
        match self {
            Self::Bot { login } | Self::Mannequin { login } | Self::User { login } => login,
            Self::EnterpriseTeam { name } | Self::Team { name } => name,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
enum ProjectFieldRaw {
    #[serde(rename = "ProjectV2ItemFieldTextValue")]
    Text { field: FieldRaw, text: Option<String> },
    #[serde(rename = "ProjectV2ItemFieldDateValue")]
    Date { field: FieldRaw, date: Option<String> },
    #[serde(rename = "ProjectV2ItemFieldSingleSelectValue")]
    SingleSelect { field: FieldRaw, name: Option<String> },
    #[serde(rename = "ProjectV2ItemFieldNumberValue")]
    Number { field: FieldRaw, number: Option<f64> },
    #[serde(rename = "ProjectV2ItemFieldIterationValue")]
    Iteration {
        field: FieldRaw,
        title: Option<String>,
        #[serde(rename = "startDate")]
        start_date: Option<String>,
        duration: Option<u64>,
    },
    #[serde(rename = "ProjectV2ItemFieldMultiSelectValue")]
    MultiSelect { field: FieldRaw, value: String },
    #[serde(rename = "ProjectV2ItemFieldLabelValue")]
    Label { field: FieldRaw, labels: Connection<NamedRaw> },
    #[serde(rename = "ProjectV2ItemFieldMilestoneValue")]
    Milestone { field: FieldRaw, milestone: Option<MilestoneRaw> },
    #[serde(rename = "ProjectV2ItemFieldPullRequestValue")]
    PullRequest {
        field: FieldRaw,
        #[serde(rename = "pullRequests")]
        pull_requests: Connection<NumberReferenceRaw>,
    },
    #[serde(rename = "ProjectV2ItemFieldRepositoryValue")]
    Repository { field: FieldRaw, repository: Option<NameRepositoryRaw> },
    #[serde(rename = "ProjectV2ItemFieldReviewerValue")]
    Reviewer { field: FieldRaw, reviewers: Connection<ReviewerRaw> },
    #[serde(rename = "ProjectV2ItemFieldUserValue")]
    User { field: FieldRaw, users: Connection<LoginRaw> },
    #[serde(rename = "ProjectV2ItemIssueFieldValue")]
    IssueField {
        field: FieldRaw,
        #[serde(rename = "issueFieldValue")]
        issue_field_value: Option<IssueFieldValueRaw>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
enum IssueFieldValueRaw {
    #[serde(rename = "IssueFieldDateValue")]
    Date { value: String },
    #[serde(rename = "IssueFieldTextValue")]
    Text { value: String },
    #[serde(rename = "IssueFieldNumberValue")]
    Number { value: f64 },
    #[serde(rename = "IssueFieldSingleSelectValue")]
    SingleSelect { name: String },
    #[serde(rename = "IssueFieldMultiSelectValue")]
    MultiSelect { value: String },
}

#[derive(Debug, Clone, Deserialize)]
struct LinkedBranchRaw {
    r#ref: Option<BranchRefRaw>,
}

#[derive(Debug, Clone, Deserialize)]
struct BranchRefRaw {
    name: String,
    repository: NameRepositoryRaw,
}

#[derive(Debug, Clone, Deserialize)]
struct CommentRaw {
    id: String,
    #[serde(rename = "fullDatabaseId")]
    full_database_id: String,
    author: Option<ActorRaw>,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    body: String,
    #[serde(rename = "isMinimized")]
    is_minimized: bool,
    #[serde(rename = "minimizedReason")]
    minimized_reason: Option<String>,
    #[serde(rename = "isPinned", default)]
    is_pinned: bool,
}

impl CommentRaw {
    fn snapshot(self, repository: &str, work_item_number: u64) -> CommentSnapshot {
        CommentSnapshot {
            node_id: self.id,
            database_id: self.full_database_id,
            repository: repository.to_owned(),
            work_item_number,
            author: self.author.map(Into::into),
            created_at: self.created_at,
            updated_at: self.updated_at,
            body: (!self.is_minimized).then_some(self.body),
            minimized: self.is_minimized,
            minimized_reason: self.minimized_reason,
            pinned: self.is_pinned,
            deleted: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ReviewRaw {
    id: String,
    #[serde(rename = "fullDatabaseId")]
    full_database_id: String,
    author: Option<ActorRaw>,
    state: String,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "submittedAt")]
    submitted_at: Option<String>,
    #[serde(rename = "updatedAt")]
    updated_at: String,
    body: String,
    #[serde(rename = "isMinimized")]
    is_minimized: bool,
    #[serde(rename = "minimizedReason")]
    minimized_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReviewThreadRaw {
    id: String,
    path: String,
    line: Option<u64>,
    #[serde(rename = "startLine")]
    start_line: Option<u64>,
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    #[serde(rename = "resolvedBy")]
    resolved_by: Option<ActorRaw>,
    #[serde(rename = "isCollapsed")]
    is_collapsed: bool,
    #[serde(rename = "isOutdated")]
    is_outdated: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct DuplicateEventRaw {
    duplicate: DuplicateTargetRaw,
    canonical: DuplicateTargetRaw,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "__typename")]
enum DuplicateTargetRaw {
    Issue {
        id: String,
        number: u64,
        title: String,
        state: String,
        #[serde(rename = "stateReason")]
        state_reason: Option<String>,
        repository: NameRepositoryRaw,
    },
    PullRequest {
        id: String,
        number: u64,
        title: String,
        state: String,
        repository: NameRepositoryRaw,
    },
}

impl DuplicateTargetRaw {
    fn reference(self) -> WorkItemReference {
        match self {
            Self::Issue { id, number, title, state, state_reason, repository } => {
                ReferenceRaw { id, number, title, state, state_reason, repository }
                    .issue_reference()
            }
            Self::PullRequest { id, number, title, state, repository } => {
                ReferenceRaw { id, number, title, state, state_reason: None, repository }
                    .pr_reference()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueMarker {
    repository: RepositoryRaw,
    core: IssueCoreRaw,
    association_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PullRequestMarker {
    repository: RepositoryRaw,
    core: PullRequestCoreRaw,
    association_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct NodeConnectionData<T> {
    node: Option<NodeConnectionRoot<T>>,
}

#[derive(Debug, Deserialize)]
struct NodeConnectionRoot<T> {
    items: Connection<T>,
}

#[derive(Debug, Serialize)]
struct NodeConnectionVariables<'a> {
    id: &'a str,
    cursor: Option<&'a str>,
    #[serde(rename = "pageSize")]
    page_size: usize,
}

async fn read_issue_core(
    client: &GitHubClient,
    locator: &WorkItemLocator,
) -> Result<(RepositoryRaw, IssueCoreRaw), ContextError> {
    let variables = serde_json::json!({
        "owner": locator.repository.owner,
        "name": locator.repository.name,
        "number": locator.number,
    });
    let data: IssueRootData = client.graphql(ISSUE_CORE_QUERY, &variables).await?;
    let repository = data.repository.ok_or_else(|| ContextError::Missing {
        kind: "repository",
        target: locator.repository.to_string(),
    })?;
    let issue = repository
        .issue
        .ok_or_else(|| ContextError::Missing { kind: "Issue", target: locator.to_string() })?;
    Ok((RepositoryRaw { id: repository.id, name_with_owner: repository.name_with_owner }, issue))
}

async fn read_pr_core(
    client: &GitHubClient,
    locator: &WorkItemLocator,
) -> Result<(RepositoryRaw, PullRequestCoreRaw), ContextError> {
    let variables = serde_json::json!({
        "owner": locator.repository.owner,
        "name": locator.repository.name,
        "number": locator.number,
    });
    let data: PullRequestRootData = client.graphql(PR_CORE_QUERY, &variables).await?;
    let repository = data.repository.ok_or_else(|| ContextError::Missing {
        kind: "repository",
        target: locator.repository.to_string(),
    })?;
    let pull_request = repository
        .pull_request
        .ok_or_else(|| ContextError::Missing { kind: "PR", target: locator.to_string() })?;
    Ok((
        RepositoryRaw { id: repository.id, name_with_owner: repository.name_with_owner },
        pull_request,
    ))
}

async fn read_issue_marker(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
) -> Result<IssueMarker, ContextError> {
    let (repository, core) = read_issue_core(client, locator).await?;
    let mut association_ids = paginate_issue::<ReferenceRaw>(
        client,
        locator,
        page_size,
        ISSUE_ASSOCIATED_PRS_QUERY,
        "associated pull requests",
    )
    .await?
    .into_iter()
    .map(|reference| reference.id)
    .collect::<Vec<_>>();
    association_ids.sort();
    Ok(IssueMarker { repository, core, association_ids })
}

async fn read_pr_marker(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
) -> Result<PullRequestMarker, ContextError> {
    let (repository, core) = read_pr_core(client, locator).await?;
    let mut association_ids = paginate_pr::<ReferenceRaw>(
        client,
        locator,
        page_size,
        PR_ASSOCIATED_ISSUES_QUERY,
        "associated issues",
    )
    .await?
    .into_iter()
    .map(|reference| reference.id)
    .collect::<Vec<_>>();
    association_ids.sort();
    Ok(PullRequestMarker { repository, core, association_ids })
}

async fn paginate_issue<T: for<'de> Deserialize<'de>>(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
    query: &str,
    connection: &'static str,
) -> Result<Vec<T>, ContextError> {
    let mut output = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_PAGES {
        let variables = ConnectionVariables {
            owner: &locator.repository.owner,
            name: &locator.repository.name,
            number: locator.number,
            cursor: cursor.as_deref(),
            page_size,
        };
        let data: IssueConnectionData<T> = client.graphql(query, &variables).await?;
        let page = data
            .repository
            .and_then(|repository| repository.issue)
            .ok_or_else(|| ContextError::Missing { kind: "Issue", target: locator.to_string() })?
            .items;
        output.extend(page.nodes);
        if !page.page_info.has_next_page {
            return Ok(output);
        }
        let next = page.page_info.end_cursor.ok_or_else(|| ContextError::Incomplete {
            connection,
            target: locator.to_string(),
            reason: "hasNextPage was true without an endCursor".into(),
        })?;
        if !seen.insert(next.clone()) {
            return Err(ContextError::Incomplete {
                connection,
                target: locator.to_string(),
                reason: "GitHub repeated a pagination cursor".into(),
            });
        }
        cursor = Some(next);
    }
    Err(ContextError::Incomplete {
        connection,
        target: locator.to_string(),
        reason: format!("exceeded the bounded limit of {MAX_PAGES} pages × {page_size} nodes"),
    })
}

async fn paginate_pr<T: for<'de> Deserialize<'de>>(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
    query: &str,
    connection: &'static str,
) -> Result<Vec<T>, ContextError> {
    let mut output = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_PAGES {
        let variables = ConnectionVariables {
            owner: &locator.repository.owner,
            name: &locator.repository.name,
            number: locator.number,
            cursor: cursor.as_deref(),
            page_size,
        };
        let data: PullRequestConnectionData<T> = client.graphql(query, &variables).await?;
        let page = data
            .repository
            .and_then(|repository| repository.pull_request)
            .ok_or_else(|| ContextError::Missing { kind: "PR", target: locator.to_string() })?
            .items;
        output.extend(page.nodes);
        if !page.page_info.has_next_page {
            return Ok(output);
        }
        let next = page.page_info.end_cursor.ok_or_else(|| ContextError::Incomplete {
            connection,
            target: locator.to_string(),
            reason: "hasNextPage was true without an endCursor".into(),
        })?;
        if !seen.insert(next.clone()) {
            return Err(ContextError::Incomplete {
                connection,
                target: locator.to_string(),
                reason: "GitHub repeated a pagination cursor".into(),
            });
        }
        cursor = Some(next);
    }
    Err(ContextError::Incomplete {
        connection,
        target: locator.to_string(),
        reason: format!("exceeded the bounded limit of {MAX_PAGES} pages × {page_size} nodes"),
    })
}

async fn paginate_node<T: for<'de> Deserialize<'de>>(
    client: &GitHubClient,
    id: &str,
    page_size: usize,
    query: &str,
    connection: &'static str,
) -> Result<Vec<T>, ContextError> {
    let mut output = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    for _ in 0..MAX_PAGES {
        let variables = NodeConnectionVariables { id, cursor: cursor.as_deref(), page_size };
        let data: NodeConnectionData<T> = client.graphql(query, &variables).await?;
        let page = data
            .node
            .ok_or_else(|| ContextError::Missing { kind: "node", target: id.to_owned() })?;
        output.extend(page.items.nodes);
        if !page.items.page_info.has_next_page {
            return Ok(output);
        }
        let next = page.items.page_info.end_cursor.ok_or_else(|| ContextError::Incomplete {
            connection,
            target: id.to_owned(),
            reason: "hasNextPage was true without an endCursor".into(),
        })?;
        if !seen.insert(next.clone()) {
            return Err(ContextError::Incomplete {
                connection,
                target: id.to_owned(),
                reason: "GitHub repeated a pagination cursor".into(),
            });
        }
        cursor = Some(next);
    }
    Err(ContextError::Incomplete {
        connection,
        target: id.to_owned(),
        reason: format!("exceeded the bounded limit of {MAX_PAGES} pages × {page_size} nodes"),
    })
}

#[allow(clippy::too_many_lines)]
async fn read_issue_snapshot(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    marker: IssueMarker,
    page_size: usize,
) -> Result<IssueSnapshot, ContextError> {
    let core = marker.core;
    let (
        assignees,
        labels,
        project_items,
        linked_branches,
        sub_issues,
        blocked_by,
        blocking,
        associated_prs,
        comments,
        duplicate_events,
    ) = tokio::try_join!(
        paginate_issue::<ActorRaw>(client, locator, page_size, ISSUE_ASSIGNEES_QUERY, "assignees"),
        paginate_issue::<LabelRaw>(client, locator, page_size, ISSUE_LABELS_QUERY, "labels"),
        read_issue_project_items(client, locator, page_size),
        paginate_issue::<LinkedBranchRaw>(
            client,
            locator,
            page_size,
            ISSUE_LINKED_BRANCHES_QUERY,
            "linked branches"
        ),
        paginate_issue::<ReferenceRaw>(
            client,
            locator,
            page_size,
            ISSUE_SUB_ISSUES_QUERY,
            "sub-issues"
        ),
        paginate_issue::<ReferenceRaw>(
            client,
            locator,
            page_size,
            ISSUE_BLOCKED_BY_QUERY,
            "blocked-by"
        ),
        paginate_issue::<ReferenceRaw>(
            client,
            locator,
            page_size,
            ISSUE_BLOCKING_QUERY,
            "blocking"
        ),
        paginate_issue::<ReferenceRaw>(
            client,
            locator,
            page_size,
            ISSUE_ASSOCIATED_PRS_QUERY,
            "associated pull requests"
        ),
        paginate_issue::<CommentRaw>(client, locator, page_size, ISSUE_COMMENTS_QUERY, "comments"),
        paginate_issue::<DuplicateEventRaw>(
            client,
            locator,
            page_size,
            ISSUE_DUPLICATES_QUERY,
            "duplicate events"
        ),
    )?;

    let mut assignees = assignees.into_iter().map(Into::into).collect::<Vec<Actor>>();
    assignees
        .sort_by(|left, right| left.login.cmp(&right.login).then(left.node_id.cmp(&right.node_id)));
    let mut labels = labels.into_iter().map(|label| label.name).collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    let projects = materialize_projects(client, project_items, page_size).await?;
    let mut linked_branches = linked_branches
        .into_iter()
        .filter_map(|branch| branch.r#ref)
        .map(|branch| format!("{}:{}", branch.repository.name_with_owner, branch.name))
        .collect::<Vec<_>>();
    linked_branches.sort();
    linked_branches.dedup();
    let mut sub_issues =
        sub_issues.into_iter().map(ReferenceRaw::issue_reference).collect::<Vec<_>>();
    sort_references(&mut sub_issues);
    let mut blocked_by =
        blocked_by.into_iter().map(ReferenceRaw::issue_reference).collect::<Vec<_>>();
    sort_references(&mut blocked_by);
    let mut blocking = blocking.into_iter().map(ReferenceRaw::issue_reference).collect::<Vec<_>>();
    sort_references(&mut blocking);
    let mut associated_prs =
        associated_prs.into_iter().map(ReferenceRaw::pr_reference).collect::<Vec<_>>();
    sort_references(&mut associated_prs);
    let mut comments = comments
        .into_iter()
        .map(|comment| comment.snapshot(&locator.repository.name_with_owner(), locator.number))
        .collect::<Vec<_>>();
    comments.sort_by(|left, right| {
        left.created_at.cmp(&right.created_at).then(left.node_id.cmp(&right.node_id))
    });
    let mut duplicate_pairs = duplicate_events
        .into_iter()
        .map(|event| (event.duplicate.reference(), event.canonical.reference()))
        .collect::<Vec<_>>();
    duplicate_pairs.sort_by(|left, right| {
        left.0
            .repository
            .cmp(&right.0.repository)
            .then(left.0.number.cmp(&right.0.number))
            .then(left.1.repository.cmp(&right.1.repository))
            .then(left.1.number.cmp(&right.1.number))
    });

    Ok(IssueSnapshot {
        node_id: core.id,
        database_id: core.full_database_id,
        repository_node_id: marker.repository.id,
        repository: marker.repository.name_with_owner,
        number: core.number,
        title: core.title,
        body: core.body,
        state: core.state,
        state_reason: core.state_reason,
        updated_at: core.updated_at,
        author: core.author.map(Into::into),
        issue_type: core.issue_type.map(|issue_type| issue_type.name),
        assignees,
        labels,
        milestone: core.milestone.map(|milestone| milestone.title),
        projects,
        linked_branches,
        parent: core.parent.map(ReferenceRaw::issue_reference),
        sub_issues,
        blocked_by,
        blocking,
        duplicate_pairs,
        associated_prs,
        comments,
    })
}

#[allow(clippy::too_many_lines)]
async fn read_pr_snapshot(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    marker: PullRequestMarker,
    page_size: usize,
) -> Result<PullRequestSnapshot, ContextError> {
    let core = marker.core;
    let (assignees, labels, project_items, associated_issue_refs, comments, reviews, threads) = tokio::try_join!(
        paginate_pr::<ActorRaw>(client, locator, page_size, PR_ASSIGNEES_QUERY, "assignees"),
        paginate_pr::<LabelRaw>(client, locator, page_size, PR_LABELS_QUERY, "labels"),
        read_pr_project_items(client, locator, page_size),
        paginate_pr::<ReferenceRaw>(
            client,
            locator,
            page_size,
            PR_ASSOCIATED_ISSUES_QUERY,
            "associated issues"
        ),
        paginate_pr::<CommentRaw>(client, locator, page_size, PR_COMMENTS_QUERY, "conversation"),
        paginate_pr::<ReviewRaw>(client, locator, page_size, PR_REVIEWS_QUERY, "reviews"),
        paginate_pr::<ReviewThreadRaw>(
            client,
            locator,
            page_size,
            PR_REVIEW_THREADS_QUERY,
            "review threads"
        ),
    )?;

    let mut associated_issue_locators = associated_issue_refs
        .into_iter()
        .map(|reference| {
            let repository = reference.repository.name_with_owner.parse::<RepositoryName>()?;
            Ok(WorkItemLocator { repository, number: reference.number })
        })
        .collect::<Result<Vec<_>, GitHubError>>()?;
    associated_issue_locators.sort_by(|left, right| {
        left.repository.cmp(&right.repository).then(left.number.cmp(&right.number))
    });
    associated_issue_locators.dedup();
    let mut associated_issues = Vec::with_capacity(associated_issue_locators.len());
    for issue_locator in associated_issue_locators {
        let issue = if issue_locator.repository == locator.repository {
            materialize_issue(client, &issue_locator, page_size).await?
        } else {
            let associated_client = client.for_repository(&issue_locator.repository).await?;
            materialize_issue(&associated_client, &issue_locator, page_size).await?
        };
        if !issue.associated_prs.iter().any(|associated_pr| {
            associated_pr.repository == locator.repository.name_with_owner()
                && associated_pr.number == locator.number
        }) {
            return Err(ContextError::Incomplete {
                connection: "native association",
                target: locator.to_string(),
                reason: format!("{issue_locator} did not expose the reciprocal PR edge"),
            });
        }
        associated_issues.push(issue);
    }

    let mut assignees = assignees.into_iter().map(Into::into).collect::<Vec<Actor>>();
    assignees
        .sort_by(|left, right| left.login.cmp(&right.login).then(left.node_id.cmp(&right.node_id)));
    let mut labels = labels.into_iter().map(|label| label.name).collect::<Vec<_>>();
    labels.sort();
    labels.dedup();
    let projects = materialize_projects(client, project_items, page_size).await?;
    let mut conversation = comments
        .into_iter()
        .map(|comment| comment.snapshot(&locator.repository.name_with_owner(), locator.number))
        .collect::<Vec<_>>();
    conversation.sort_by(|left, right| {
        left.created_at.cmp(&right.created_at).then(left.node_id.cmp(&right.node_id))
    });
    let mut reviews = reviews
        .into_iter()
        .map(|review| ReviewSnapshot {
            node_id: review.id,
            database_id: review.full_database_id,
            author: review.author.map(Into::into),
            state: review.state,
            created_at: review.submitted_at.unwrap_or(review.created_at),
            updated_at: review.updated_at,
            body: review.body,
            minimized: review.is_minimized,
            minimized_reason: review.minimized_reason,
        })
        .collect::<Vec<_>>();
    reviews.sort_by(|left, right| {
        left.created_at.cmp(&right.created_at).then(left.node_id.cmp(&right.node_id))
    });
    let mut review_threads = Vec::with_capacity(threads.len());
    for thread in threads {
        let mut thread_comments = paginate_node::<CommentRaw>(
            client,
            &thread.id,
            page_size,
            REVIEW_THREAD_COMMENTS_QUERY,
            "review thread comments",
        )
        .await?
        .into_iter()
        .map(|comment| comment.snapshot(&locator.repository.name_with_owner(), locator.number))
        .collect::<Vec<_>>();
        thread_comments.sort_by(|left, right| {
            left.created_at.cmp(&right.created_at).then(left.node_id.cmp(&right.node_id))
        });
        review_threads.push(ReviewThreadSnapshot {
            node_id: thread.id,
            path: thread.path,
            line: thread.line,
            start_line: thread.start_line,
            resolved: thread.is_resolved,
            resolved_by: thread.resolved_by.map(Into::into),
            collapsed: thread.is_collapsed,
            outdated: thread.is_outdated,
            comments: thread_comments,
        });
    }
    review_threads.sort_by(|left, right| {
        let left_created = left.comments.first().map(|comment| comment.created_at.as_str());
        let right_created = right.comments.first().map(|comment| comment.created_at.as_str());
        left_created.cmp(&right_created).then(left.node_id.cmp(&right.node_id))
    });

    Ok(PullRequestSnapshot {
        node_id: core.id,
        database_id: core.full_database_id,
        repository_node_id: marker.repository.id,
        repository: marker.repository.name_with_owner,
        number: core.number,
        title: core.title,
        body: core.body,
        state: core.state,
        draft: core.is_draft,
        merged: core.merged,
        updated_at: core.updated_at,
        author: core.author.map(Into::into),
        base_ref: core.base_ref_name,
        head_repository: core.head_repository.map(|repository| repository.name_with_owner),
        head_ref: core.head_ref_name,
        assignees,
        labels,
        milestone: core.milestone.map(|milestone| milestone.title),
        projects,
        associated_issues,
        conversation,
        reviews,
        review_threads,
    })
}

async fn materialize_projects(
    client: &GitHubClient,
    project_items: Vec<ProjectRaw>,
    page_size: usize,
) -> Result<Vec<ProjectEntry>, ContextError> {
    let mut projects = Vec::with_capacity(project_items.len());
    for project_item in project_items {
        let values = paginate_node::<ProjectFieldRaw>(
            client,
            &project_item.id,
            page_size,
            PROJECT_FIELD_VALUES_QUERY,
            "project field values",
        )
        .await?;
        let mut fields = values
            .into_iter()
            .map(project_field)
            .collect::<Result<Vec<_>, ContextError>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        fields.sort_by(|left, right| left.name.cmp(&right.name).then(left.value.cmp(&right.value)));
        projects.push(ProjectEntry { title: project_item.project.title, fields });
    }
    projects.sort_by(|left, right| left.title.cmp(&right.title));
    Ok(projects)
}

async fn read_issue_project_items(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
) -> Result<Vec<ProjectRaw>, ContextError> {
    if client.projects_v2_enabled() {
        paginate_issue(client, locator, page_size, ISSUE_PROJECTS_QUERY, "project items").await
    } else {
        Ok(Vec::new())
    }
}

async fn read_pr_project_items(
    client: &GitHubClient,
    locator: &WorkItemLocator,
    page_size: usize,
) -> Result<Vec<ProjectRaw>, ContextError> {
    if client.projects_v2_enabled() {
        paginate_pr(client, locator, page_size, PR_PROJECTS_QUERY, "project items").await
    } else {
        Ok(Vec::new())
    }
}

fn project_field(value: ProjectFieldRaw) -> Result<Option<ProjectField>, ContextError> {
    let (field, value) = match value {
        ProjectFieldRaw::Text { field, text } => (field, text),
        ProjectFieldRaw::Date { field, date } => (field, date),
        ProjectFieldRaw::SingleSelect { field, name } => (field, name),
        ProjectFieldRaw::Number { field, number } => (field, number.map(format_number)),
        ProjectFieldRaw::Iteration { field, title, start_date, duration } => {
            let value = title.map(|title| match (start_date, duration) {
                (Some(start), Some(duration)) => format!("{title} ({start}, {duration} days)"),
                _ => title,
            });
            (field, value)
        }
        ProjectFieldRaw::MultiSelect { field, value } => {
            (field, (!value.is_empty()).then_some(value))
        }
        ProjectFieldRaw::Label { field, labels } => {
            ensure_nested_complete(&labels.page_info, "project label values", &field.name)?;
            let mut names = labels.nodes.into_iter().map(|label| label.name).collect::<Vec<_>>();
            names.sort();
            (field, (!names.is_empty()).then(|| names.join(", ")))
        }
        ProjectFieldRaw::Milestone { field, milestone } => {
            (field, milestone.map(|milestone| milestone.title))
        }
        ProjectFieldRaw::PullRequest { field, pull_requests } => {
            ensure_nested_complete(
                &pull_requests.page_info,
                "project pull-request values",
                &field.name,
            )?;
            let mut values = pull_requests
                .nodes
                .into_iter()
                .map(|pull_request| {
                    format!("{}#{}", pull_request.repository.name_with_owner, pull_request.number)
                })
                .collect::<Vec<_>>();
            values.sort();
            (field, (!values.is_empty()).then(|| values.join(", ")))
        }
        ProjectFieldRaw::Repository { field, repository } => {
            (field, repository.map(|repository| repository.name_with_owner))
        }
        ProjectFieldRaw::Reviewer { field, reviewers } => {
            ensure_nested_complete(&reviewers.page_info, "project reviewer values", &field.name)?;
            let mut values =
                reviewers.nodes.into_iter().map(ReviewerRaw::display_name).collect::<Vec<_>>();
            values.sort();
            (field, (!values.is_empty()).then(|| values.join(", ")))
        }
        ProjectFieldRaw::User { field, users } => {
            ensure_nested_complete(&users.page_info, "project user values", &field.name)?;
            let mut values = users.into_nodes_logins();
            values.sort();
            (field, (!values.is_empty()).then(|| values.join(", ")))
        }
        ProjectFieldRaw::IssueField { field, issue_field_value } => {
            let value = issue_field_value.map(|value| match value {
                IssueFieldValueRaw::Date { value }
                | IssueFieldValueRaw::Text { value }
                | IssueFieldValueRaw::MultiSelect { value } => value,
                IssueFieldValueRaw::Number { value } => format_number(value),
                IssueFieldValueRaw::SingleSelect { name } => name,
            });
            (field, value)
        }
    };
    Ok(value
        .filter(|value| !value.is_empty())
        .map(|value| ProjectField { name: field.name, value }))
}

trait LoginConnectionExt {
    fn into_nodes_logins(self) -> Vec<String>;
}

impl LoginConnectionExt for Connection<LoginRaw> {
    fn into_nodes_logins(self) -> Vec<String> {
        self.nodes.into_iter().map(|user| user.login).collect()
    }
}

fn ensure_nested_complete(
    page_info: &PageInfo,
    connection: &'static str,
    target: &str,
) -> Result<(), ContextError> {
    if page_info.has_next_page {
        Err(ContextError::Incomplete {
            connection,
            target: target.to_owned(),
            reason: format!("nested value exceeded the bounded {MAX_PAGE_SIZE}-node selection"),
        })
    } else {
        Ok(())
    }
}

fn format_number(number: f64) -> String {
    if number.fract() == 0.0 { format!("{number:.0}") } else { number.to_string() }
}

fn sort_references(references: &mut [WorkItemReference]) {
    references.sort_by(|left, right| {
        left.repository.cmp(&right.repository).then(left.number.cmp(&right.number))
    });
}
