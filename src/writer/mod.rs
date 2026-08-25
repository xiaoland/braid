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
mod helpers;
mod prepare;

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
    helpers::require_configured_repository(config, &request.target)?;
    let profile = config.profile(&request.profile_id)?;
    let github = GitHubClient::connect(&config.github, &request.target.repository)
        .await
        .context("cannot authenticate Braid App comment writer")?;
    let work_item = github.issue_or_pull_request(request.target.number).await?;
    let role = helpers::role_for_target(profile, work_item.kind())?;
    github.require_write_permissions(&[if work_item.kind() == "pr" {
        "pull_requests"
    } else {
        "issues"
    }])?;
    let body = helpers::render_agent_comment(profile, role, &request.body)?;
    let idempotency = match request.request_id {
        Some(request_id) => {
            helpers::validate_request_id(&request_id)?;
            request_id
        }
        None => body.clone(),
    };
    let request_key = format!(
        "comment_create\0{}\0{}\0{}\0{}",
        request.target.repository, request.target.number, profile.id, idempotency
    );
    let request_digest = helpers::digest(&format!(
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
    match helpers::claim_or_wait(store, &receipt.intent_id).await? {
        helpers::Claim::Done(receipt) => Ok(receipt),
        helpers::Claim::Owned(receipt) => {
            let result = comment::converge_comment(&github, request.target.number, &receipt).await;
            helpers::settle_write(store, &receipt.intent_id, result)?;
            helpers::completed_receipt(store, &receipt.intent_id)
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
    let receipt =
        prepare::prepare_pull_request(config, store, &github, repository, request).await?;
    match helpers::claim_or_wait(store, &receipt.write.intent_id).await? {
        helpers::Claim::Done(_) => {
            helpers::completed_implementation_receipt(store, &receipt.write.intent_id)
        }
        helpers::Claim::Owned(_) => {
            let result = ensure::converge_pull_request(config, &github, store, &receipt).await;
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
            helpers::completed_implementation_receipt(store, &receipt.write.intent_id)
        }
    }
}
