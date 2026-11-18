#![allow(clippy::large_futures)]
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use sha2::{Digest, Sha256};

use crate::{
    config::{Config, Profile},
    context,
    github::{GitHubClient, GitHubError, PullRequest, WorkItemLocator},
    store::{
        GhWriteReceipt, ImplementationProgress, ImplementationRequestReceipt, NewGhWriteIntent,
        NewImplementationRequest, SchedulerPolicy, StoreActor,
    },
};

mod comment;
mod edit;
mod handoff;
pub use edit::{EditRequest, edit, read_for_edit};
pub use handoff::{HandoffRequest, handoff};
mod ensure;
pub(crate) mod helpers;
mod prepare;

const COMMENT_BODY_LIMIT: usize = 60_000;

pub enum Invocation {
    Agent(String),
    Operator,
}

pub struct CommentCreateRequest {
    pub invocation: Invocation,
    pub target: WorkItemLocator,
    pub profile_id: String,
    pub body: String,
    pub request_id: Option<String>,
}

pub struct PullRequestEnsureRequest {
    pub invocation: Invocation,
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
    let source = resolve_source(config, Some(&profile.id), &request.invocation)?;
    let role = match source.as_ref().map(|s| s.role.as_str()) {
        Some("issue_agent") => "Issue Agent",
        Some("pr_implementation_agent") => "PR Implementation Agent",
        Some(_) => bail!("unknown source role"),
        None => "Operator",
    };
    let scope = source.as_ref().map_or("operator", |s| s.assignment_id.as_str());
    let payload = serde_json::to_string(&AcceptedRequest::Comment {
        profile: profile.clone(),
        body: request.body.trim().to_owned(),
    })?;
    let idempotency = request.request_id.unwrap_or_else(|| helpers::digest(&payload));
    helpers::validate_request_id(&idempotency)?;
    store
        .prepare_gh_write(NewGhWriteIntent {
            request_key: format!(
                "comment_create\0{}\0{}\0{}\0{}\0{}",
                request.target.repository, request.target.number, profile.id, scope, idempotency
            ),
            operation: "comment_create",
            repository: request.target.repository.to_string(),
            target: request.target.to_string(),
            profile_id: profile.id.clone(),
            role: role.into(),
            source,
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
    let source = resolve_source(config, None, &request.invocation)?;
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
    Handoff { quiet_seconds: u64, event_threshold: u32 },
    Comment { profile: Profile, body: String },
    Edit { profile: Profile, expected: String, title: String, body: String },
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
    if receipt.operation == "pr_handoff" {
        return handoff::execute(config, github, &receipt).await;
    }
    if receipt.operation == "body_edit" {
        return edit::execute(config, store, github, &receipt, mutation_possible).await;
    }
    if receipt.operation == "comment_create" {
        let target: WorkItemLocator = receipt.target.parse()?;
        let item = github.issue_or_pull_request(target.number).await?;
        if let Ok(AcceptedRequest::Comment { profile, body }) =
            serde_json::from_str(&receipt.payload)
        {
            let role = helpers::publication_role(&profile, receipt.source.as_ref(), item.kind())
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
        if let Some(remote) = comment::recover_comment(github, target.number, &receipt).await? {
            return Ok(remote);
        }
        edit::authorize(config, github, &receipt, false).await?;
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
                config,
                store,
                github,
                &receipt,
                profile,
                PullRequestEnsureRequest {
                    invocation: Invocation::Operator,
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

pub(crate) fn resolve_source(
    config: &Config,
    profile: Option<&str>,
    invocation: &Invocation,
) -> Result<Option<crate::store::publication_source::PublicationSource>> {
    let cwd = std::env::current_dir()?;
    match invocation {
        Invocation::Agent(turn) => {
            let source = crate::store::publication_source::resolve_turn(
                config.runtime.database(),
                &cwd,
                profile,
                turn,
            )?;
            let materials = config.profile_materials(config.profile(&source.profile_id)?)?;
            if u64::try_from(source.profile_revision)? != materials.revision {
                bail!("publication Profile materials differ from the running assignment");
            }
            Ok(Some(source))
        }
        Invocation::Operator => {
            if std::env::var_os("BRAID_AGENT").is_some()
                || crate::store::publication_source::is_runtime_worktree(
                    config.runtime.database(),
                    &cwd,
                )?
            {
                bail!("operator entry is unavailable inside an Agent runtime worktree");
            }
            Ok(None)
        }
    }
}

pub(crate) fn check_source(config: &Config, receipt: &GhWriteReceipt) -> Result<(), GitHubError> {
    if let Some(owner) = &receipt.execution_owner {
        crate::store::check_publication_owner(config.runtime.database(), owner)
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
    }
    if let Some(source) = &receipt.source {
        crate::store::publication_source::check(config.runtime.database(), source)
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
        let profile =
            config.profile(&source.profile_id).map_err(|e| GitHubError::GraphQl(e.to_string()))?;
        let materials =
            config.profile_materials(profile).map_err(|e| GitHubError::GraphQl(e.to_string()))?;
        if u64::try_from(source.profile_revision).ok() != Some(materials.revision) {
            return Err(GitHubError::GraphQl("publication Profile materials changed".into()));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;
