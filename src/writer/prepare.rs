#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn prepare_pull_request(
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
    helpers::validate_branch(&head_ref)?;
    if head_ref == base_ref {
        bail!("Implementation Request head must differ from the default branch {base_ref:?}");
    }
    let profile = config.profile(&config.profile_selection.default_pr_profile)?;
    if !profile.has_tag("pr") {
        bail!("default PR Profile {:?} is not tagged pr", profile.id);
    }
    let profile_digest = helpers::digest(&serde_json::to_string(profile)?);
    let request_key = format!("pr_ensure\0{}\0{}", config.github.repository, request.comment_id);
    let request_digest = helpers::digest(&format!(
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
