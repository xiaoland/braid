#![allow(clippy::wildcard_imports)]
use super::helpers::{RemoteWrite, WriteFailure, WriteResult};
use super::*;

pub async fn converge_comment(
    config: &Config,
    github: &GitHubClient,
    issue_number: u64,
    receipt: &GhWriteReceipt,
    mutation_possible: &mut bool,
) -> WriteResult {
    if receipt.attempts > 0 {
        let candidates = github
            .issue_comments(issue_number)
            .await?
            .into_iter()
            .filter(|comment| {
                comment.body.as_deref() == Some(receipt.payload.as_str())
                    && comment.created_at >= receipt.created_at
                    && receipt
                        .recovery_until
                        .as_ref()
                        .is_none_or(|until| comment.created_at <= *until)
                    && comment
                        .user
                        .as_ref()
                        .is_some_and(|actor| actor.node_id == github.identity().actor_node_id)
            })
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [comment] => {
                return Ok(RemoteWrite {
                    database_id: comment.id.to_string(),
                    node_id: comment.node_id.clone(),
                });
            }
            [] => {}
            _ => return Err(WriteFailure::Ambiguous("multiple matching App comments".into())),
        }
    }
    let item = github.issue_or_pull_request(issue_number).await?;
    github.require_write_permissions(&[if item.kind() == "pr" {
        "pull_requests"
    } else {
        "issues"
    }])?;
    check_source(config, receipt)?;
    *mutation_possible = true;
    let comment = github.create_issue_comment(issue_number, &receipt.payload).await?;
    Ok(RemoteWrite { database_id: comment.id.to_string(), node_id: comment.node_id })
}
