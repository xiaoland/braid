#![allow(clippy::wildcard_imports)]
use super::*;

pub(crate) async fn drain_one_write(store: &StoreActor, github: &GitHubClient) {
    let write = match store.claim_github_write() {
        Ok(write) => write,
        Err(error) => {
            tracing::error!(%error, "cannot claim GitHub write");
            return;
        }
    };
    let Some(write) = write else { return };
    if write.repository != github.identity().repository {
        let _ = store.finish_github_write(
            write.intent_id,
            "rejected",
            None,
            None,
            Some("write repository does not match connected installation".into()),
        );
        return;
    }
    let recovered_comment = if write.operation == "comment_create" && write.lifecycle == "uncertain"
    {
        recover_uncertain_comment(github, &write).await
    } else {
        Ok(None)
    };
    let result = match recovered_comment {
        Err(error) => Err(error),
        Ok(Some(comment)) => Ok(comment),
        Ok(None) => match write.operation.as_str() {
            "reaction_add" => github
                .add_reaction(&write.target_kind, &write.target_database_id, &write.content)
                .await
                .map(|id| AppliedWrite { database_id: Some(id.to_string()), node_id: None })
                .map_err(OutboxWriteError::from),
            "reaction_delete" => {
                match write
                    .remote_database_id
                    .as_deref()
                    .and_then(|value| value.parse::<u64>().ok())
                {
                    Some(reaction_id) => github
                        .delete_reaction(&write.target_kind, &write.target_database_id, reaction_id)
                        .await
                        .map(|()| AppliedWrite {
                            database_id: Some(reaction_id.to_string()),
                            node_id: None,
                        })
                        .map_err(OutboxWriteError::from),
                    None => bail_unknown_write("reaction_delete without a reaction ID"),
                }
            }
            "comment_create" => match write.target_database_id.parse::<u64>() {
                Ok(issue_number) => github
                    .create_issue_comment(issue_number, &write.content)
                    .await
                    .map(AppliedWrite::from)
                    .map_err(OutboxWriteError::from),
                Err(_) => Err(OutboxWriteError::GitHub(crate::github::GitHubError::GraphQl(
                    "invalid status Issue number".into(),
                ))),
            },
            "comment_update" => github
                .update_issue_comment(&write.target_database_id, &write.content)
                .await
                .map(AppliedWrite::from)
                .map_err(OutboxWriteError::from),
            operation => bail_unknown_write(operation),
        },
    };
    match result {
        Ok(remote_id) => {
            if let Err(error) = store.finish_github_write(
                write.intent_id,
                "applied",
                remote_id.database_id,
                remote_id.node_id,
                None,
            ) {
                tracing::error!(%error, "cannot acknowledge GitHub write");
            }
        }
        Err(error) => {
            let lifecycle = match &error {
                OutboxWriteError::Ambiguous(_) => "ambiguous",
                OutboxWriteError::GitHub(error) if error.is_unavailable() => "uncertain",
                OutboxWriteError::GitHub(_) => "rejected",
            };
            if let Err(store_error) = store.finish_github_write(
                write.intent_id,
                lifecycle,
                None,
                None,
                Some(error.to_string()),
            ) {
                tracing::error!(%store_error, "cannot record GitHub write failure");
            }
        }
    }
}

pub(crate) async fn recover_uncertain_comment(
    github: &GitHubClient,
    write: &crate::store::PendingGitHubWrite,
) -> Result<Option<AppliedWrite>, OutboxWriteError> {
    let issue_number = write.target_database_id.parse::<u64>().map_err(|_| {
        crate::github::GitHubError::GraphQl(
            "uncertain comment create has an invalid Issue number".into(),
        )
    })?;
    let matches = github
        .issue_comments(issue_number)
        .await?
        .into_iter()
        .filter(|comment| {
            comment.body.as_deref() == Some(write.content.as_str())
                && comment.created_at >= write.created_at
                && comment
                    .user
                    .as_ref()
                    .is_some_and(|actor| actor.node_id == github.identity().actor_node_id)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [comment] => Ok(Some(AppliedWrite {
            database_id: Some(comment.id.to_string()),
            node_id: Some(comment.node_id.clone()),
        })),
        _ => Err(OutboxWriteError::Ambiguous(
            "uncertain comment create matches multiple App comments".into(),
        )),
    }
}

pub(crate) struct AppliedWrite {
    database_id: Option<String>,
    node_id: Option<String>,
}

pub(crate) enum OutboxWriteError {
    GitHub(crate::github::GitHubError),
    Ambiguous(String),
}

impl From<crate::github::GitHubError> for OutboxWriteError {
    fn from(error: crate::github::GitHubError) -> Self {
        Self::GitHub(error)
    }
}

impl std::fmt::Display for OutboxWriteError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GitHub(error) => error.fmt(formatter),
            Self::Ambiguous(message) => formatter.write_str(message),
        }
    }
}

impl From<CreatedIssueComment> for AppliedWrite {
    fn from(comment: CreatedIssueComment) -> Self {
        Self { database_id: Some(comment.id.to_string()), node_id: Some(comment.node_id) }
    }
}

pub(crate) fn bail_unknown_write(operation: &str) -> Result<AppliedWrite, OutboxWriteError> {
    Err(OutboxWriteError::GitHub(crate::github::GitHubError::GraphQl(format!(
        "unsupported outbox operation {operation:?}"
    ))))
}
