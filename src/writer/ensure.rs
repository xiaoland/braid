#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn converge_pull_request(
    config: &Config,
    github: &GitHubClient,
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
    mutation_possible: &mut bool,
) -> Result<PullRequest, GitHubError> {
    let request_marker = format!("Implementation request: {}", receipt.write.target);
    let mut pull_request = if let Some(number) = receipt.pull_request_number {
        let pr = github.pull_request(number).await?;
        if pr.head.reference != receipt.head_ref
            || pr.base.reference != receipt.base_ref
            || !pr.body.as_deref().is_some_and(|body| body.contains(&request_marker))
        {
            return Err(GitHubError::GraphQl(
                "frozen PR no longer matches implementation request".into(),
            ));
        }
        Some(pr)
    } else {
        helpers::unique_request_pull_request(
            github,
            &receipt.head_ref,
            &receipt.base_ref,
            &request_marker,
        )
        .await?
    };
    if pull_request.is_none() {
        helpers::ensure_head(config, github, store, receipt, mutation_possible).await?;
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
        check_source(config, &receipt.write)?;
        *mutation_possible = true;
        github
            .create_draft_pull_request(&title, &body, &receipt.head_ref, &receipt.base_ref)
            .await?
    };
    if !pull_request.state.eq_ignore_ascii_case("open") {
        return Err(GitHubError::GraphQl("implementation request PR is no longer open; refusing to create a replacement or reactivate it".into()));
    }
    if matches!(receipt.stage.as_str(), "planned" | "head_ready" | "pull_request_ready") {
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
    }
    helpers::wait_for_native_association(github, pull_request.number, receipt.issue_number).await?;
    if receipt.stage != "activation_pending" {
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
    }
    check_source(config, &receipt.write)?;
    helpers::record_pr_activation(config, github, store, receipt, &pull_request)?;
    Ok(pull_request)
}
