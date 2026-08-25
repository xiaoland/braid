#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn converge_pull_request(
    config: &Config,
    github: &GitHubClient,
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
) -> Result<PullRequest, GitHubError> {
    let request_marker = format!("Implementation request: {}", receipt.write.target);
    let mut pull_request = helpers::unique_request_pull_request(
        github,
        &receipt.head_ref,
        &receipt.base_ref,
        &request_marker,
    )
    .await?;
    if pull_request.is_none() {
        helpers::ensure_head(github, store, receipt).await?;
        pull_request = helpers::unique_request_pull_request(
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
        let title = helpers::bounded_title(receipt.issue_number, &receipt.issue_title);
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
        .map_err(|error| helpers::store_as_github_error(&error))?;
    helpers::wait_for_native_association(github, pull_request.number, receipt.issue_number).await?;
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
        .map_err(|error| helpers::store_as_github_error(&error))?;
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
        .map_err(|error| helpers::store_as_github_error(&error))?;
    helpers::record_pr_activation(config, github, store, receipt, &pull_request)?;
    Ok(pull_request)
}
