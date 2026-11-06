//! Canonical collaboration state, independent of the provider session transport.
use std::sync::Arc;

use anyhow::Result;

use crate::{
    builtin::BuiltinClient,
    context::{self, Actor, CanonicalContext, IssueSnapshot, PullRequestSnapshot},
    github::{GitHubClient, WorkItemLocator},
};

pub(crate) enum CollaborationBackend {
    GitHub(Arc<GitHubClient>),
    Builtin(Arc<BuiltinClient>),
}

impl CollaborationBackend {
    pub(crate) fn is_builtin(&self) -> bool {
        matches!(self, Self::Builtin(_))
    }

    pub(crate) async fn materialize_issue(
        &self,
        locator: &WorkItemLocator,
    ) -> Result<IssueSnapshot> {
        match self {
            Self::GitHub(github) => {
                Ok(Box::pin(context::materialize_issue(github, locator, 100)).await?)
            }
            Self::Builtin(client) => match client.context(locator.number).await? {
                CanonicalContext::Issue(issue) => Ok(issue),
                CanonicalContext::PullRequest(_) => {
                    anyhow::bail!("built-in item #{} is not an Issue", locator.number)
                }
            },
        }
    }

    pub(crate) async fn materialize_pull_request(
        &self,
        locator: &WorkItemLocator,
    ) -> Result<PullRequestSnapshot> {
        match self {
            Self::GitHub(github) => {
                Ok(Box::pin(context::materialize_pull_request(github, locator, 100)).await?)
            }
            Self::Builtin(client) => match client.context(locator.number).await? {
                CanonicalContext::PullRequest(pr) => Ok(pr),
                CanonicalContext::Issue(_) => {
                    anyhow::bail!("built-in item #{} is not a PR", locator.number)
                }
            },
        }
    }

    /// Built-in PR generations start from the authority's frozen candidate commit.
    pub(crate) async fn pr_worktree_ref(&self, number: u64, head_ref: &str) -> Result<String> {
        match self {
            Self::GitHub(_) => Ok(head_ref.to_owned()),
            Self::Builtin(client) => client.candidate(number).await,
        }
    }
    pub(crate) async fn ensure_grant(
        &self,
        session_id: &str,
        number: u64,
        actor: &str,
    ) -> Result<()> {
        match self {
            Self::GitHub(_) => Ok(()),
            Self::Builtin(client) => client.ensure_grant(session_id, number, actor).await,
        }
    }
    pub(crate) async fn revoke_session(&self, session_id: &str) -> Result<()> {
        match self {
            Self::GitHub(_) => Ok(()),
            Self::Builtin(client) => client.revoke_session(session_id).await,
        }
    }

    pub(crate) async fn default_branch(&self) -> Result<String> {
        match self {
            Self::GitHub(github) => Ok(github.repository_details().await?.default_branch),
            Self::Builtin(client) => client.default_branch().await,
        }
    }

    pub(crate) fn assignee_matches(&self, actor: &Actor, profile_id: &str) -> bool {
        match self {
            Self::GitHub(github) => {
                actor.node_id == github.identity().actor_node_id
                    || actor.login == github.identity().actor_login
            }
            Self::Builtin(_) => actor.login == profile_id,
        }
    }
}
