#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn prepare_pull_request(
    config: &Config,
    store: &StoreActor,
    github: &GitHubClient,
    accepted: &GhWriteReceipt,
    profile: Profile,
    request: PullRequestEnsureRequest,
) -> Result<ImplementationRequestReceipt, GitHubError> {
    let repository = request.repository.clone();
    prepare_plan(config, store, github, accepted, profile, request, repository).await.map_err(|e| {
        e.downcast::<GitHubError>().unwrap_or_else(|e| GitHubError::GraphQl(e.to_string()))
    })
}

async fn prepare_plan(
    config: &Config,
    store: &StoreActor,
    github: &GitHubClient,
    accepted: &GhWriteReceipt,
    profile: Profile,
    request: PullRequestEnsureRequest,
    repository: crate::github::RepositoryName,
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
    if let Some(source) = &accepted.source {
        let (source_repository, source_kind, source_number) =
            crate::store::publication_source::work_item(config.runtime.database(), source)?;
        if source_repository != repository.to_string()
            || source_kind != "issue"
            || source_number != issue_number
        {
            bail!("initial PR ensure must originate from its own Issue source");
        }
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
                .strip_prefix(&format!("{repository}:"))
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
    store
        .prepare_implementation_request(NewImplementationRequest {
            write: NewGhWriteIntent {
                request_key: format!("pr_ensure\0{}\0{}", repository, request.comment_id),
                operation: "pr_ensure",
                repository: repository.to_string(),
                target: accepted.target.clone(),
                profile_id: profile.id.clone(),
                role: "PR Implementation Agent".into(),
                payload: accepted.payload.clone(),
                source: accepted.source.clone(),
                request_digest: helpers::digest(&accepted.payload),
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
