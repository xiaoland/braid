//! Explicit delivery of an Issue's canonical design to an existing PR generation.
use super::{AcceptedRequest, Invocation, check_source, helpers, resolve_source};
use crate::{
    config::Config,
    github::{GitHubClient, GitHubError, WorkItemLocator},
    store::{GhWriteReceipt, NewGhWriteIntent, StoreActor},
};
use anyhow::{Result, bail};

pub struct HandoffRequest {
    pub invocation: Invocation,
    pub target: WorkItemLocator,
    pub request_id: String,
}

pub fn handoff(
    config: &Config,
    store: &StoreActor,
    request: &HandoffRequest,
) -> Result<GhWriteReceipt> {
    helpers::require_configured_repository(config, &request.target)?;
    config.github()?;
    helpers::validate_request_id(&request.request_id)?;
    let source = resolve_source(config, None, &request.invocation)?
        .ok_or_else(|| anyhow::anyhow!("handoff requires a bound Issue Agent, not Operator"))?;
    if source.role != "issue_agent" {
        bail!("handoff requires an Issue Agent");
    }
    let (repository, kind, _) =
        crate::store::publication_source::work_item(config.runtime.database(), &source)?;
    if repository != request.target.repository.to_string() || kind != "issue" {
        bail!("handoff must stay within the source Issue repository");
    }
    let payload = serde_json::to_string(&AcceptedRequest::Handoff {
        quiet_seconds: config.scheduler.quiet_seconds,
        event_threshold: config.scheduler.event_threshold,
    })?;
    store
        .prepare_gh_write(NewGhWriteIntent {
            request_key: format!(
                "pr_handoff\0{}\0{}\0{}",
                source.assignment_id, source.generation, request.request_id
            ),
            operation: "pr_handoff",
            repository,
            target: request.target.to_string(),
            profile_id: source.profile_id.clone(),
            role: "Issue Agent".into(),
            request_digest: helpers::digest(&request.target.to_string()),
            payload,
            source: Some(source),
        })
        .map_err(Into::into)
}

pub(super) async fn execute(
    config: &Config,
    github: &GitHubClient,
    receipt: &GhWriteReceipt,
) -> helpers::WriteResult {
    check_source(config, receipt)?;
    crate::store::handoff::validate(config.runtime.database(), receipt)
        .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
    let source =
        receipt.source.as_ref().ok_or_else(|| GitHubError::GraphQl("missing source".into()))?;
    let (repository, _, number) =
        crate::store::publication_source::work_item(config.runtime.database(), source)
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
    if github.identity().repository != repository {
        return Err(GitHubError::GraphQl("handoff installation repository mismatch".into()).into());
    }
    github.require_write_permissions(&["issues", "pull_requests"])?;
    let issue = github.issue_or_pull_request(number).await?;
    let target: WorkItemLocator = receipt.target.parse()?;
    let pr = github.issue_or_pull_request(target.number).await?;
    // GitHub's canonical OPEN state excludes merged PRs (which are CLOSED).
    if issue.kind() != "issue"
        || !issue.state.eq_ignore_ascii_case("open")
        || pr.kind() != "pr"
        || !pr.state.eq_ignore_ascii_case("open")
    {
        return Err(GitHubError::GraphQl(
            "handoff requires an OPEN Issue and OPEN unmerged PR".into(),
        )
        .into());
    }
    if !github
        .pull_request_closing_issues(target.number)
        .await?
        .iter()
        .any(|i| i.repository == repository && i.number == number)
    {
        return Err(GitHubError::GraphQl(
            "handoff requires GitHub-confirmed direct association, not body text or references"
                .into(),
        )
        .into());
    }
    check_source(config, receipt)?;
    Ok(helpers::RemoteWrite { database_id: target.number.to_string(), node_id: pr.node_id })
}
