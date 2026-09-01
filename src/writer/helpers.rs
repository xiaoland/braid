#![allow(clippy::wildcard_imports)]
use super::*;

pub fn require_configured_repository(config: &Config, target: &WorkItemLocator) -> Result<()> {
    if target.repository.to_string() != config.github.repository {
        bail!(
            "target repository {} does not match configured repository {}",
            target.repository,
            config.github.repository
        );
    }
    Ok(())
}

pub fn role_for_target(profile: &Profile, kind: &str) -> Result<&'static str> {
    match kind {
        "issue" if profile.has_tag("issue") => Ok("Issue Agent"),
        "pr" if profile.has_tag("pr") => Ok("PR Implementation Agent"),
        _ => bail!("Profile {:?} is not tagged for {kind}", profile.id),
    }
}

pub fn render_agent_comment(profile: &Profile, role: &str, body: &str) -> Result<String> {
    let attribution = format!("> **Braid Agent · {}**\n> {}", profile.display_name, role);
    let mut body = body.trim();
    while let Some(remainder) = body.strip_prefix(&attribution) {
        body = remainder.trim_start();
    }
    if body.is_empty() {
        bail!("Agent comment body must not be empty");
    }
    let rendered = format!("{attribution}\n\n{body}");
    if rendered.len() > COMMENT_BODY_LIMIT {
        bail!(
            "Agent comment is {} bytes; Braid's safe limit is {COMMENT_BODY_LIMIT}",
            rendered.len()
        );
    }
    Ok(rendered)
}

pub async fn unique_request_pull_request(
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

pub async fn wait_for_native_association(
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

pub async fn ensure_head(
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

pub fn record_pr_activation(
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
                delivery_guid: format!(
                    "braid-pr-ensure-{repository}-{}",
                    receipt.comment_database_id
                ),
                event_name: "braid".into(),
                action: Some("pr_ensure".into()),
                repository_node_id: github.identity().repository_node_id.clone(),
                repository: repository.clone(),
                work_item_node_id: Some(pull_request.node_id.clone()),
                work_item_kind: Some("pr"),
                work_item_number: Some(pull_request.number),
                work_item_state: Some(pull_request.state.clone()),
                object_node_id: None,
                object_version: None,
                object_digest: None,
                visible_body: None,
                actor_node_id: Some(github.identity().actor_node_id.clone()),
                actor_login: Some(github.identity().actor_login.clone()),
                kind: crate::store::EventKind::Assign,
                detail: Some("pr_ensure"),
                cross_surface_invalidation: false,
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

pub enum Claim {
    Owned(GhWriteReceipt),
    Done(GhWriteReceipt),
}

pub async fn claim_or_wait(store: &StoreActor, intent_id: &str) -> Result<Claim> {
    for _ in 0..CLAIM_POLLS {
        let receipt = store
            .gh_write_receipt(intent_id.to_owned())?
            .context("Braid write state disappeared while waiting for convergence")?;
        match receipt.lifecycle.as_str() {
            "applied" => return Ok(Claim::Done(receipt)),
            "rejected" | "conflict" | "ambiguous" => {
                bail!(
                    "braid gh {} for {} is {}: {}",
                    receipt.operation,
                    receipt.target,
                    receipt.lifecycle,
                    receipt.last_error.as_deref().unwrap_or("no detail")
                );
            }
            "pending" | "uncertain" | "sending" => {
                if store.claim_gh_write(intent_id.to_owned())? {
                    let claimed = store
                        .gh_write_receipt(intent_id.to_owned())?
                        .context("claimed Braid write state disappeared")?;
                    return Ok(Claim::Owned(claimed));
                }
            }
            lifecycle => bail!("unsupported braid gh write lifecycle {lifecycle:?}"),
        }
        tokio::time::sleep(CLAIM_POLL_INTERVAL).await;
    }
    bail!("timed out waiting for a concurrent braid gh write to converge")
}

pub struct RemoteWrite {
    pub database_id: String,
    pub node_id: String,
    pub url: String,
}

pub enum WriteFailure {
    GitHub(GitHubError),
    Ambiguous(String),
}

impl WriteFailure {
    pub fn is_unavailable(&self) -> bool {
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

pub type WriteResult = Result<RemoteWrite, WriteFailure>;

pub fn settle_write(store: &StoreActor, intent_id: &str, result: WriteResult) -> Result<()> {
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

pub fn completed_receipt(store: &StoreActor, intent_id: &str) -> Result<GhWriteReceipt> {
    let receipt = store
        .gh_write_receipt(intent_id.to_owned())?
        .context("Braid write state disappeared before completion")?;
    if receipt.lifecycle == "applied" {
        Ok(receipt)
    } else {
        bail!(
            "braid gh {} for {} is {}: {}",
            receipt.operation,
            receipt.target,
            receipt.lifecycle,
            receipt.last_error.as_deref().unwrap_or("no detail")
        )
    }
}

pub fn completed_implementation_receipt(
    store: &StoreActor,
    intent_id: &str,
) -> Result<ImplementationRequestReceipt> {
    let receipt = store
        .implementation_request_receipt(intent_id.to_owned())?
        .context("Implementation Request state disappeared before completion")?;
    if receipt.write.lifecycle == "applied" {
        Ok(receipt)
    } else {
        bail!(
            "braid gh pr ensure for Issue #{} is {}: {}",
            receipt.issue_number,
            receipt.write.lifecycle,
            receipt.write.last_error.as_deref().unwrap_or("no detail")
        )
    }
}

pub fn bounded_title(issue_number: u64, title: &str) -> String {
    let title = title.chars().take(180).collect::<String>();
    format!("Implement #{issue_number}: {title}")
}

pub fn validate_request_id(request_id: &str) -> Result<()> {
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

pub fn validate_branch(branch: &str) -> Result<()> {
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

pub fn digest(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub fn store_as_github_error(error: &crate::store::StoreError) -> GitHubError {
    GitHubError::GraphQl(format!("local durable state rejected PR convergence: {error}"))
}
