#![allow(clippy::large_futures)]
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

mod comment;
mod ensure;
pub(crate) mod helpers;
mod prepare;

const COMMENT_BODY_LIMIT: usize = 60_000;

pub struct CommentCreateRequest {
    pub target: WorkItemLocator,
    pub profile_id: String,
    pub body: String,
    pub request_id: Option<String>,
}

pub struct PullRequestEnsureRequest {
    pub repository: crate::github::RepositoryName,
    pub comment_id: u64,
    pub head: Option<String>,
}

pub fn create_comment(
    config: &Config,
    store: &StoreActor,
    request: CommentCreateRequest,
) -> Result<GhWriteReceipt> {
    helpers::require_configured_repository(config, &request.target)?;
    let profile = config.profile(&request.profile_id)?;
    if config.profile_repository(profile) != request.target.repository.to_string() {
        bail!(
            "Profile {:?} serves {}, not {}",
            profile.id,
            config.profile_repository(profile),
            request.target.repository
        );
    }
    config.github()?;
    // Snapshot the local profile; remote kind and permissions remain unvalidated.
    helpers::render_agent_comment(profile, "PR Implementation Agent", &request.body)?;
    let payload = serde_json::to_string(&AcceptedRequest::Comment {
        profile: profile.clone(),
        body: request.body.trim().to_owned(),
    })?;
    let idempotency = request.request_id.unwrap_or_else(|| helpers::digest(&payload));
    helpers::validate_request_id(&idempotency)?;
    store
        .prepare_gh_write(NewGhWriteIntent {
            request_key: format!(
                "comment_create\0{}\0{}\0{}\0{}",
                request.target.repository, request.target.number, profile.id, idempotency
            ),
            operation: "comment_create",
            repository: request.target.repository.to_string(),
            target: request.target.to_string(),
            profile_id: profile.id.clone(),
            role: "pending_validation".into(),
            source: source(config, Some(&profile.id))?,
            request_digest: helpers::digest(&payload),
            payload,
        })
        .map_err(Into::into)
}

pub fn ensure_pull_request(
    config: &Config,
    store: &StoreActor,
    request: PullRequestEnsureRequest,
) -> Result<GhWriteReceipt> {
    helpers::require_configured_repository(
        config,
        &WorkItemLocator { repository: request.repository.clone(), number: 1 },
    )?;
    config.github()?;
    if request.comment_id == 0 {
        bail!("comment ID must be positive");
    }
    if let Some(head) = &request.head {
        helpers::validate_branch(head)?;
    }
    let profile = config.pr_profile_for_repository(&request.repository.to_string())?;
    if !profile.has_tag("pr") {
        bail!("PR Profile must be tagged pr");
    }
    let source = source(config, None)?;
    if source.as_ref().is_some_and(|source| source.role != "issue_agent") {
        bail!("initial PR ensure requires an Issue Agent source");
    }
    let payload = serde_json::to_string(&AcceptedRequest::Ensure {
        comment_id: request.comment_id,
        head: request.head,
        profile: profile.clone(),
    })?;
    store
        .prepare_gh_write(NewGhWriteIntent {
            request_key: format!("pr_ensure\0{}\0{}", request.repository, request.comment_id),
            operation: "pr_ensure",
            repository: request.repository.to_string(),
            target: format!(
                "https://github.com/{}/issues/comments/{}",
                request.repository, request.comment_id
            ),
            profile_id: profile.id.clone(),
            role: "PR Implementation Agent".into(),
            source,
            request_digest: helpers::digest(&payload),
            payload,
        })
        .map_err(Into::into)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub(crate) enum AcceptedRequest {
    Comment { profile: Profile, body: String },
    Ensure { comment_id: u64, head: Option<String>, profile: Profile },
}

/// The runtime is the sole mutation owner. Acceptance does not authorize a remote target.
pub(crate) async fn execute(
    config: &Config,
    store: &StoreActor,
    github: &GitHubClient,
    write: &crate::store::PendingGitHubWrite,
    mutation_possible: &mut bool,
) -> helpers::WriteResult {
    use helpers::{RemoteWrite, WriteFailure};
    let mut receipt = store
        .gh_write_receipt(write.intent_id.clone())
        .map_err(|e| helpers::store_as_github_error(&e))?
        .ok_or_else(|| GitHubError::GraphQl("accepted write disappeared".into()))?;
    receipt.recovery_until = Some(write.recovery_until.clone());
    receipt.execution_owner = Some(write.owner.clone());
    receipt.attempts = u64::from(write.lifecycle == "uncertain");
    if receipt.operation == "comment_create" {
        let target: WorkItemLocator = receipt.target.parse()?;
        let item = github.issue_or_pull_request(target.number).await?;
        if receipt.source.as_ref().is_some_and(|source| {
            source.role
                != if item.kind() == "issue" { "issue_agent" } else { "pr_implementation_agent" }
        }) {
            return Err(GitHubError::GraphQl(
                "publication target role does not match source assignment".into(),
            )
            .into());
        }
        if receipt.role == "pending_validation" {
            let AcceptedRequest::Comment { profile, body } = serde_json::from_str(&receipt.payload)
                .map_err(|e| GitHubError::GraphQl(e.to_string()))?
            else {
                return Err(WriteFailure::GitHub(GitHubError::GraphQl(
                    "invalid comment request".into(),
                )));
            };
            let role = helpers::role_for_target(&profile, item.kind())
                .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
            receipt.payload = helpers::render_agent_comment(&profile, role, &body)
                .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
        } else {
            helpers::role_for_target(
                config
                    .profile(&receipt.profile_id)
                    .map_err(|e| GitHubError::GraphQl(e.to_string()))?,
                item.kind(),
            )
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
        }
        comment::converge_comment(config, github, target.number, &receipt, mutation_possible).await
    } else {
        let mut plan = if let Some(plan) = store
            .implementation_request_receipt(receipt.intent_id.clone())
            .map_err(|e| helpers::store_as_github_error(&e))?
        {
            plan
        } else {
            let AcceptedRequest::Ensure { comment_id, head, profile } =
                serde_json::from_str(&receipt.payload)
                    .map_err(|e| GitHubError::GraphQl(e.to_string()))?
            else {
                return Err(WriteFailure::GitHub(GitHubError::GraphQl(
                    "invalid ensure request".into(),
                )));
            };
            prepare::prepare_pull_request(
                store,
                github,
                &receipt,
                profile,
                PullRequestEnsureRequest {
                    repository: receipt.repository.parse()?,
                    comment_id,
                    head,
                },
            )
            .await?
        };
        plan.write.execution_owner = Some(write.owner.clone());
        if plan.pr_profile_id != receipt.profile_id {
            return Err(GitHubError::GraphQl("frozen Profile mismatch".into()).into());
        }
        github.require_write_permissions(&["pull_requests", "contents"])?;
        check_source(config, &receipt)?;
        let pr =
            ensure::converge_pull_request(config, github, store, &plan, mutation_possible).await?;
        Ok(RemoteWrite { database_id: pr.id.to_string(), node_id: pr.node_id })
    }
}

fn source(
    config: &Config,
    profile: Option<&str>,
) -> Result<Option<crate::store::publication_source::PublicationSource>> {
    crate::store::publication_source::resolve(
        config.runtime.database(),
        &std::env::current_dir()?,
        profile,
        std::env::var_os("BRAID_AGENT").is_some(),
    )
    .map_err(Into::into)
}

pub(crate) fn check_source(config: &Config, receipt: &GhWriteReceipt) -> Result<(), GitHubError> {
    if let Some(owner) = &receipt.execution_owner {
        crate::store::check_publication_owner(config.runtime.database(), owner)
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
    }
    if let Some(source) = &receipt.source {
        crate::store::publication_source::check(config.runtime.database(), source)
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
