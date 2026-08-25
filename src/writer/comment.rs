#![allow(clippy::wildcard_imports)]
use super::*;
use crate::writer::helpers::{RemoteWrite, WriteFailure, WriteResult};

pub async fn converge_comment(
    github: &GitHubClient,
    issue_number: u64,
    receipt: &GhWriteReceipt,
) -> WriteResult {
    if receipt.attempts > 0 {
        let candidates = github
            .issue_comments(issue_number)
            .await?
            .into_iter()
            .filter(|comment| {
                comment.body.as_deref() == Some(receipt.payload.as_str())
                    && comment.created_at >= receipt.created_at
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
                    url: comment.html_url.clone(),
                });
            }
            [] => {}
            _ => return Err(WriteFailure::Ambiguous("multiple matching App comments".into())),
        }
    }
    let comment = github.create_issue_comment(issue_number, &receipt.payload).await?;
    Ok(RemoteWrite {
        database_id: comment.id.to_string(),
        node_id: comment.node_id,
        url: format!(
            "https://github.com/{}/issues/{issue_number}#issuecomment-{}",
            github.identity().repository,
            comment.id
        ),
    })
}
