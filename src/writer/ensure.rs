#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn converge_pull_request(
    config: &Config,
    github: &GitHubClient,
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
    mutation_possible: &mut bool,
) -> Result<PullRequest, GitHubError> {
    let request_marker = format!("Implementation request: {}", receipt.root_target);
    let mut pull_request = if let Some(number) = receipt.pull_request_number {
        let pr = github.pull_request(number).await?;
        if pr.head.reference != receipt.head_ref
            || pr.base.reference != receipt.base_ref
            || receipt.pull_request_node_id.as_deref() != Some(pr.node_id.as_str())
        {
            return Err(GitHubError::GraphQl(
                "frozen PR no longer matches implementation request".into(),
            ));
        }
        if receipt.activation_delivered && receipt.write.intent_id == receipt.root_intent_id {
            return Ok(pr);
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
    let source = github.issue_or_pull_request(receipt.issue_number).await?;
    if source.node_id != receipt.issue_node_id
        || source.kind() != "issue"
        || !source.state.eq_ignore_ascii_case("open")
    {
        return Err(GitHubError::GraphQl(
            "implementation source Issue is no longer the same open work item".into(),
        ));
    }
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
        record_stage(store, receipt, &pull_request, "pull_request_ready")?;
    }
    if matches!(receipt.stage.as_str(), "associated" | "activation_pending") {
        if !github.pull_request_closing_issues(pull_request.number).await?.iter().any(|edge| {
            edge.repository == receipt.write.repository && edge.number == receipt.issue_number
        }) {
            return Err(GitHubError::GraphQl(
                "managed implementation is no longer directly associated with its source Issue"
                    .into(),
            ));
        }
    } else {
        helpers::wait_for_native_association(github, pull_request.number, receipt.issue_number)
            .await?;
    }
    if receipt.stage != "activation_pending" {
        record_stage(store, receipt, &pull_request, "associated")?;
        record_stage(store, receipt, &pull_request, "activation_pending")?;
    }
    if !receipt.activation_delivered {
        check_execution_owner(config, &receipt.write)?;
        helpers::record_pr_activation(config, github, store, receipt, &pull_request).await?;
    }
    Ok(pull_request)
}

fn record_stage(
    store: &StoreActor,
    receipt: &ImplementationRequestReceipt,
    pr: &PullRequest,
    stage: &'static str,
) -> Result<(), GitHubError> {
    store
        .record_implementation_progress(
            receipt.write.intent_id.clone(),
            ImplementationProgress {
                stage,
                bootstrap_commit_sha: None,
                pull_request_database_id: Some(pr.id),
                pull_request_node_id: Some(pr.node_id.clone()),
                pull_request_number: Some(pr.number),
            },
        )
        .map_err(|error| helpers::store_as_github_error(&error))
}
