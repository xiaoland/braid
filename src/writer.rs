use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest, Sha256};

use crate::{
    config::{Config, Profile},
    context,
    github::{GitHubClient, GitHubError, PullRequest, WorkItemLocator},
    store::{
        GhWriteReceipt, ImplementationProgress, ImplementationRequestReceipt, IngressEvent,
        NewGhWriteIntent, NewImplementationRequest, SchedulerPolicy, StoreActor,
    },
};

const COMMENT_BODY_LIMIT: usize = 60_000;
const CLAIM_POLLS: usize = 240;
const CLAIM_POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct CommentCreateRequest {
    pub target: WorkItemLocator,
    pub profile_id: String,
    pub body: String,
    pub request_id: Option<String>,
}

pub struct PullRequestEnsureRequest {
    pub comment_id: u64,
    pub head: Option<String>,
}

pub async fn create_comment(
    config: &Config,
    store: &StoreActor,
    request: CommentCreateRequest,
) -> Result<GhWriteReceipt> {
    require_configured_repository(config, &request.target)?;
    let profile = config.profile(&request.profile_id)?;
    let github = GitHubClient::connect(&config.github, &request.target.repository)
        .await
        .context("cannot authenticate Braid App comment writer")?;
    let work_item = github.issue_or_pull_request(request.target.number).await?;
    let role = role_for_target(profile, work_item.kind())?;
    github.require_write_permissions(&[if work_item.kind() == "pr" {
        "pull_requests"
    } else {
        "issues"
    }])?;
    let body = render_agent_comment(profile, role, &request.body)?;
    let idempotency = match request.request_id {
        Some(request_id) => {
            validate_request_id(&request_id)?;
            request_id
        }
        None => digest(&body),
    };
    let request_key = format!(
        "comment_create\0{}\0{}\0{}\0{}",
        request.target.repository, request.target.number, profile.id, idempotency
    );
    let request_digest = digest(&format!(
        "comment_create\0{}\0{}\0{}\0{}\0{}",
        request.target.repository, request.target.number, profile.id, role, body
    ));
    let receipt = store.prepare_gh_write(NewGhWriteIntent {
        request_key,
        operation: "comment_create",
        repository: request.target.repository.to_string(),
        target: request.target.to_string(),
        profile_id: profile.id.clone(),
        role: role.into(),
        payload: body,
        request_digest,
    })?;
    match claim_or_wait(store, &receipt.intent_id).await? {
        Claim::Done(receipt) => Ok(receipt),
        Claim::Owned(receipt) => {
            let result = converge_comment(&github, request.target.number, &receipt).await;
            settle_write(store, &receipt.intent_id, result)?;
            completed_receipt(store, &receipt.intent_id)
        }
    }
}

pub async fn ensure_pull_request(
    config: &Config,
    store: &StoreActor,
    request: PullRequestEnsureRequest,
) -> Result<ImplementationRequestReceipt> {
    let repository = config.github.repository.parse()?;
    let github = GitHubClient::connect(&config.github, &repository)
        .await
        .context("cannot authenticate Braid App PR writer")?;
    let receipt = prepare_pull_request(config, store, &github, repository, request).await?;
    match claim_or_wait(store, &receipt.write.intent_id).await? {
        Claim::Done(_) => completed_implementation_receipt(store, &receipt.write.intent_id),
        Claim::Owned(_) => {
            let result = converge_pull_request(config, &github, store, &receipt).await;
            match result {
                Ok(pull_request) => store.finish_gh_write(
                    receipt.write.intent_id.clone(),
                    "applied",
                    Some(pull_request.id.to_string()),
                    Some(pull_request.node_id),
                    Some(pull_request.html_url),
                    None,
                )?,
                Err(error) => {
                    let lifecycle = if error.is_unavailable() { "uncertain" } else { "rejected" };
                    store.finish_gh_write(
                        receipt.write.intent_id.clone(),
                        lifecycle,
                        None,
                        None,
                        None,
                        Some(error.to_string()),
                    )?;
                }
            }
            completed_implementation_receipt(store, &receipt.write.intent_id)
        }
    }
}

async fn prepare_pull_request(
    config: &Config,
    store: &StoreActor,
    github: &GitHubClient,
    repository: crate::github::RepositoryName,
    request: PullRequestEnsureRequest,
) -> Result<ImplementationRequestReceipt> {
    github.require_write_permissions(&["pull_requests", "contents"])?;
    let comment = github.issue_comment(request.comment_id).await?;
    let issue_number = comment.issue_number()?;
    let issue = github.issue_or_pull_request(issue_number).await?;
    if issue.kind() != "issue" {
        bail!(
            "Implementation Request comment {} belongs to PR #{}; an Issue comment is required",
            request.comment_id,
            issue_number
        );
    }
    let repository_details = github.repository_details().await?;
    let base_ref = repository_details.default_branch;
    let issue_locator = WorkItemLocator { repository: repository.clone(), number: issue_number };
    let head_ref = if let Some(head) = request.head {
        head
    } else {
        let issue_context = context::materialize_issue(github, &issue_locator, 100).await?;
        match issue_context.linked_branches.as_slice() {
            [] => format!("braid/implementation-request-{}", request.comment_id),
            [linked] => linked
                .strip_prefix(&format!("{}:", config.github.repository))
                .map(str::to_owned)
                .with_context(|| {
                    format!(
                        "the sole Development branch {linked:?} is outside the configured repository"
                    )
                })?,
            branches => bail!(
                "Issue #{issue_number} has {} Development branches; pass --head explicitly",
                branches.len()
            ),
        }
    };
    validate_branch(&head_ref)?;
    if head_ref == base_ref {
        bail!("Implementation Request head must differ from the default branch {base_ref:?}");
    }
    let profile = config.profile(&config.profile_selection.default_pr_profile)?;
    if !profile.has_tag("pr") {
        bail!("default PR Profile {:?} is not tagged pr", profile.id);
    }
    let profile_digest = digest(&serde_json::to_string(profile)?);
    let request_key = format!("pr_ensure\0{}\0{}", config.github.repository, request.comment_id);
    let request_digest = digest(&format!(
        "pr_ensure\0{}\0{}\0{}\0{}\0{}\0{}",
        config.github.repository,
        request.comment_id,
        base_ref,
        head_ref,
        profile.id,
        profile_digest
    ));
    store
        .prepare_implementation_request(NewImplementationRequest {
            write: NewGhWriteIntent {
                request_key,
                operation: "pr_ensure",
                repository: config.github.repository.clone(),
                target: comment.html_url.clone(),
                profile_id: profile.id.clone(),
                role: "PR Implementation Agent".into(),
                payload: comment.html_url.clone(),
                request_digest,
            },
            comment_database_id: request.comment_id,
            comment_node_id: comment.node_id,
            issue_node_id: issue.node_id,
            issue_number,
            issue_title: issue.title,
            base_ref,
            head_ref,
            pr_profile_id: profile.id.clone(),
        })
        .map_err(Into::into)
}

pub fn receipt(store: &StoreActor, intent_id: &str) -> Result<GhWriteReceipt> {
    store
        .gh_write_receipt(intent_id.to_owned())?
        .with_context(|| format!("unknown braid gh receipt {intent_id}"))
}

fn require_configured_repository(config: &Config, target: &WorkItemLocator) -> Result<()> {
    if target.repository.to_string() != config.github.repository {
        bail!(
            "target repository {} does not match configured repository {}",
            target.repository,
            config.github.repository
        );
    }
    Ok(())
}

fn role_for_target(profile: &Profile, kind: &str) -> Result<&'static str> {
    match kind {
        "issue" if profile.has_tag("issue") => Ok("Issue Agent"),
        "pr" if profile.has_tag("pr") => Ok("PR Implementation Agent"),
        _ => bail!("Profile {:?} is not tagged for {kind}", profile.id),
    }
}

fn render_agent_comment(profile: &Profile, role: &str, body: &str) -> Result<String> {
    let body = body.trim();
    if body.is_empty() {
        bail!("Agent comment body must not be empty");
    }
    let rendered = format!("> **Braid Agent · {}**\n> {}\n\n{}", profile.display_name, role, body);
    if rendered.len() > COMMENT_BODY_LIMIT {
        bail!(
            "Agent comment is {} bytes; Braid's safe limit is {COMMENT_BODY_LIMIT}",
            rendered.len()
        );
    }
    Ok(rendered)
}

async fn converge_comment(
    github: &GitHubClient,
    issue_number: u64,
    receipt: &GhWriteReceipt,
) -> WriteResult {
    if receipt.attempts > 0 {
        let candidates = github
            .issue_comments(issue_number)
            .await?
            .into_iter()
            .filter(|comment| {
                comment.body.as_deref() == Some(receipt.payload.as_str())
                    && comment.created_at >= receipt.created_at
                    && comment
                        .user
                        .as_ref()
                        .is_some_and(|actor| actor.node_id == github.identity().actor_node_id)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [comment] => {
                return Ok(RemoteWrite {
                    database_id: comment.id.to_string(),
                    node_id: comment.node_id.clone(),
                    url: comment.html_url.clone(),
                });
            }
            [] => {}
            _ => return Err(WriteFailure::Ambiguous("multiple matching App comments".into())),
        }
    }
    let comment = github.create_issue_comment(issue_number, &receipt.payload).await?;
    Ok(RemoteWrite {
        database_id: comment.id.to_string(),
        node_id: comment.node_id,
        url: format!(
            "https://github.com/{}/issues/{issue_number}#issuecomment-{}",
            github.identity().repository,
            comment.id
        ),
    })
}

async fn converge_pull_request(
    config: &Config,
    github: &GitHubClient,
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
) -> Result<PullRequest, GitHubError> {
    let request_marker = format!("Implementation request: {}", receipt.write.target);
    let mut pull_request =
        unique_request_pull_request(github, &receipt.head_ref, &receipt.base_ref, &request_marker)
            .await?;
    if pull_request.is_none() {
        ensure_head(github, store, receipt).await?;
        pull_request = unique_request_pull_request(
            github,
            &receipt.head_ref,
            &receipt.base_ref,
            &request_marker,
        )
        .await?;
    }
    let pull_request = if let Some(pull_request) = pull_request {
        pull_request
    } else {
        let title = bounded_title(receipt.issue_number, &receipt.issue_title);
        let body = format!("Closes #{}\n\n{request_marker}", receipt.issue_number);
        github
            .create_draft_pull_request(&title, &body, &receipt.head_ref, &receipt.base_ref)
            .await?
    };
    store
        .record_implementation_progress(
            receipt.write.intent_id.clone(),
            ImplementationProgress {
                stage: "pull_request_ready",
                bootstrap_commit_sha: None,
                pull_request_database_id: Some(pull_request.id),
                pull_request_node_id: Some(pull_request.node_id.clone()),
                pull_request_number: Some(pull_request.number),
            },
        )
        .map_err(|error| store_as_github_error(&error))?;
    wait_for_native_association(github, pull_request.number, receipt.issue_number).await?;
    store
        .record_implementation_progress(
            receipt.write.intent_id.clone(),
            ImplementationProgress {
                stage: "associated",
                bootstrap_commit_sha: None,
                pull_request_database_id: Some(pull_request.id),
                pull_request_node_id: Some(pull_request.node_id.clone()),
                pull_request_number: Some(pull_request.number),
            },
        )
        .map_err(|error| store_as_github_error(&error))?;
    store
        .record_implementation_progress(
            receipt.write.intent_id.clone(),
            ImplementationProgress {
                stage: "activation_pending",
                bootstrap_commit_sha: None,
                pull_request_database_id: Some(pull_request.id),
                pull_request_node_id: Some(pull_request.node_id.clone()),
                pull_request_number: Some(pull_request.number),
            },
        )
        .map_err(|error| store_as_github_error(&error))?;
    record_pr_activation(config, github, store, receipt, &pull_request)?;
    Ok(pull_request)
}

fn record_pr_activation(
    config: &Config,
    github: &GitHubClient,
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
    pull_request: &PullRequest,
) -> Result<(), GitHubError> {
    let repository = &github.identity().repository;
    let reference = format!(
        "Implementation Request comment {repository}#issuecomment-{} ensured GitHub PR {repository}#{}.",
        receipt.comment_database_id, pull_request.number,
    );
    store
        .ingest_event(
            IngressEvent {
                delivery_guid: format!("braid-pr-ensure-{}", receipt.write.intent_id),
                event_name: "braid".into(),
                action: Some("pr_ensure".into()),
                repository_node_id: github.identity().repository_node_id.clone(),
                repository: repository.clone(),
                work_item_node_id: Some(pull_request.node_id.clone()),
                work_item_kind: Some("pr"),
                work_item_number: Some(pull_request.number),
                work_item_state: Some(pull_request.state.clone()),
                object_node_id: None,
                object_version: Some(receipt.write.request_digest.clone()),
                object_digest: None,
                actor_node_id: Some(github.identity().actor_node_id.clone()),
                actor_login: Some(github.identity().actor_login.clone()),
                classification: "wake",
                origin: "braid",
                reference,
                mention_candidate: false,
                reaction_target: None,
                known: true,
                raw_payload: Vec::new(),
            },
            SchedulerPolicy {
                quiet_seconds: config.scheduler.quiet_seconds,
                event_threshold: config.scheduler.event_threshold,
            },
        )
        .map(|_| ())
        .map_err(|error| store_as_github_error(&error))
}

async fn ensure_head(
    github: &GitHubClient,
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
) -> Result<(), GitHubError> {
    let base = github.git_reference(&receipt.base_ref).await?.ok_or_else(|| {
        GitHubError::GraphQl(format!("base ref {:?} does not exist", receipt.base_ref))
    })?;
    let head = github.git_reference(&receipt.head_ref).await?;
    if head.as_ref().is_some_and(|head| head.object.sha != base.object.sha) {
        record_head_ready(store, receipt, None)?;
        return Ok(());
    }
    let parent = head.as_ref().map_or(base.object.sha.as_str(), |head| head.object.sha.as_str());
    let parent_commit = github.git_commit(parent).await?;
    let message =
        format!("chore(braid): initialize implementation request {}", receipt.comment_database_id);
    let bootstrap = github
        .create_git_commit(
            &message,
            &parent_commit.tree.sha,
            parent,
            &receipt.bootstrap_authored_at,
        )
        .await?;
    let mutation = if head.is_some() {
        github.update_git_reference(&receipt.head_ref, &bootstrap.sha).await
    } else {
        github.create_git_reference(&receipt.head_ref, &bootstrap.sha).await
    };
    match mutation {
        Ok(reference) if reference.object.sha == bootstrap.sha => {
            record_head_ready(store, receipt, Some(bootstrap.sha))?;
            Ok(())
        }
        Ok(_) => Err(GitHubError::GraphQl(
            "GitHub returned a different SHA after creating the implementation head".into(),
        )),
        Err(error) if error.is_conflict() => {
            let current = github.git_reference(&receipt.head_ref).await?;
            match current {
                Some(reference) if reference.object.sha != base.object.sha => {
                    let bootstrap =
                        (reference.object.sha == bootstrap.sha).then_some(bootstrap.sha);
                    record_head_ready(store, receipt, bootstrap)?;
                    Ok(())
                }
                _ => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

fn record_head_ready(
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
    bootstrap_commit_sha: Option<String>,
) -> Result<(), GitHubError> {
    store
        .record_implementation_progress(
            receipt.write.intent_id.clone(),
            ImplementationProgress {
                stage: "head_ready",
                bootstrap_commit_sha,
                pull_request_database_id: None,
                pull_request_node_id: None,
                pull_request_number: None,
            },
        )
        .map_err(|error| store_as_github_error(&error))
}

async fn unique_request_pull_request(
    github: &GitHubClient,
    head: &str,
    base: &str,
    marker: &str,
) -> Result<Option<PullRequest>, GitHubError> {
    let pull_requests = github.open_pull_requests_for_head(head, base).await?;
    match pull_requests.as_slice() {
        [] => Ok(None),
        [pull_request]
            if pull_request.body.as_deref().is_some_and(|body| body.contains(marker)) =>
        {
            Ok(Some(pull_request.clone()))
        }
        [pull_request] => Err(GitHubError::GraphQl(format!(
            "head {head:?} already belongs to unrelated PR #{}",
            pull_request.number
        ))),
        _ => Err(GitHubError::GraphQl(format!(
            "head {head:?} resolves to multiple open pull requests"
        ))),
    }
}

async fn wait_for_native_association(
    github: &GitHubClient,
    pull_request_number: u64,
    issue_number: u64,
) -> Result<(), GitHubError> {
    for _ in 0..20 {
        let issues = github.pull_request_closing_issues(pull_request_number).await?;
        if issues.iter().any(|issue| {
            issue.repository == github.identity().repository && issue.number == issue_number
        }) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err(GitHubError::ConvergencePending(format!(
        "PR #{pull_request_number} did not expose the native association to Issue #{issue_number}"
    )))
}

enum Claim {
    Owned(GhWriteReceipt),
    Done(GhWriteReceipt),
}

async fn claim_or_wait(store: &StoreActor, intent_id: &str) -> Result<Claim> {
    for _ in 0..CLAIM_POLLS {
        let receipt = store
            .gh_write_receipt(intent_id.to_owned())?
            .with_context(|| format!("write receipt {intent_id} disappeared"))?;
        match receipt.lifecycle.as_str() {
            "applied" => return Ok(Claim::Done(receipt)),
            "rejected" | "conflict" | "ambiguous" => {
                bail!(
                    "braid gh write {} is {}: {}",
                    receipt.intent_id,
                    receipt.lifecycle,
                    receipt.last_error.as_deref().unwrap_or("no detail")
                );
            }
            "pending" | "uncertain" | "sending" => {
                if store.claim_gh_write(intent_id.to_owned())? {
                    let claimed =
                        store.gh_write_receipt(intent_id.to_owned())?.with_context(|| {
                            format!("claimed write receipt {intent_id} disappeared")
                        })?;
                    return Ok(Claim::Owned(claimed));
                }
            }
            lifecycle => bail!("unsupported braid gh write lifecycle {lifecycle:?}"),
        }
        tokio::time::sleep(CLAIM_POLL_INTERVAL).await;
    }
    bail!("timed out waiting for concurrent braid gh write {intent_id}")
}

struct RemoteWrite {
    database_id: String,
    node_id: String,
    url: String,
}

enum WriteFailure {
    GitHub(GitHubError),
    Ambiguous(String),
}

impl WriteFailure {
    fn is_unavailable(&self) -> bool {
        matches!(self, Self::GitHub(error) if error.is_unavailable())
    }
}

impl std::fmt::Display for WriteFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Ambiguous(message) => formatter.write_str(message),
        }
    }
}

impl From<GitHubError> for WriteFailure {
    fn from(error: GitHubError) -> Self {
        Self::GitHub(error)
    }
}

type WriteResult = Result<RemoteWrite, WriteFailure>;

fn settle_write(store: &StoreActor, intent_id: &str, result: WriteResult) -> Result<()> {
    match result {
        Ok(remote) => store.finish_gh_write(
            intent_id.to_owned(),
            "applied",
            Some(remote.database_id),
            Some(remote.node_id),
            Some(remote.url),
            None,
        )?,
        Err(error) => {
            let lifecycle = match error {
                WriteFailure::Ambiguous(_) => "ambiguous",
                WriteFailure::GitHub(_) if error.is_unavailable() => "uncertain",
                WriteFailure::GitHub(_) => "rejected",
            };
            store.finish_gh_write(
                intent_id.to_owned(),
                lifecycle,
                None,
                None,
                None,
                Some(error.to_string()),
            )?;
        }
    }
    Ok(())
}

fn completed_receipt(store: &StoreActor, intent_id: &str) -> Result<GhWriteReceipt> {
    let receipt = receipt(store, intent_id)?;
    if receipt.lifecycle == "applied" {
        Ok(receipt)
    } else {
        bail!(
            "braid gh write {} is {}: {}",
            receipt.intent_id,
            receipt.lifecycle,
            receipt.last_error.as_deref().unwrap_or("no detail")
        )
    }
}

fn completed_implementation_receipt(
    store: &StoreActor,
    intent_id: &str,
) -> Result<ImplementationRequestReceipt> {
    let receipt = store
        .implementation_request_receipt(intent_id.to_owned())?
        .with_context(|| format!("Implementation Request receipt {intent_id} disappeared"))?;
    if receipt.write.lifecycle == "applied" {
        Ok(receipt)
    } else {
        bail!(
            "braid gh pr ensure {} is {}: {}",
            receipt.write.intent_id,
            receipt.write.lifecycle,
            receipt.write.last_error.as_deref().unwrap_or("no detail")
        )
    }
}

fn bounded_title(issue_number: u64, title: &str) -> String {
    let title = title.chars().take(180).collect::<String>();
    format!("Implement #{issue_number}: {title}")
}

fn validate_request_id(request_id: &str) -> Result<()> {
    if request_id.is_empty()
        || request_id.len() > 160
        || !request_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-._:/".contains(character))
    {
        bail!("request ID must be 1-160 ASCII token characters: A-Z a-z 0-9 - . _ : /");
    }
    Ok(())
}

fn validate_branch(branch: &str) -> Result<()> {
    let invalid = branch.is_empty()
        || branch.starts_with(['/', '.'])
        || branch.ends_with(['/', '.'])
        || branch.to_ascii_lowercase().ends_with(".lock")
        || branch.contains("..")
        || branch.contains("@{")
        || branch.chars().any(|character| {
            character.is_control() || character.is_whitespace() || "~^:?*[\\".contains(character)
        });
    if invalid {
        bail!("invalid Git branch name {branch:?}");
    }
    Ok(())
}

fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn store_as_github_error(error: &crate::store::StoreError) -> GitHubError {
    GitHubError::GraphQl(format!("local durable state rejected PR convergence: {error}"))
}
