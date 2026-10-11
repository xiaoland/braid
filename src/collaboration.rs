//! Canonical collaboration state, independent of the provider session transport.
use std::sync::{Arc, Mutex};

use anyhow::Result;

use crate::{
    context::{self, Actor, CanonicalContext, IssueSnapshot, PullRequestSnapshot},
    github::{GitHubClient, WorkItemLocator},
    local::objects::LocalObjects,
};

pub(crate) enum CollaborationBackend {
    GitHub(Arc<GitHubClient>),
    Local { objects: Arc<Mutex<LocalObjects>>, default_branch: String },
}

impl CollaborationBackend {
    pub(crate) fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    pub(crate) async fn materialize_issue(
        &self,
        locator: &WorkItemLocator,
    ) -> Result<IssueSnapshot> {
        match self {
            Self::GitHub(github) => {
                Ok(Box::pin(context::materialize_issue(github, locator, 100)).await?)
            }
            Self::Local { objects, .. } => match objects
                .lock()
                .map_err(|_| anyhow::anyhow!("local object store lock poisoned"))?
                .context(locator.number)?
            {
                CanonicalContext::Issue(issue) => Ok(issue),
                CanonicalContext::PullRequest(_) => {
                    anyhow::bail!("local item #{} is not an Issue", locator.number)
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
            Self::Local { objects, .. } => match objects
                .lock()
                .map_err(|_| anyhow::anyhow!("local object store lock poisoned"))?
                .context(locator.number)?
            {
                CanonicalContext::PullRequest(pr) => Ok(pr),
                CanonicalContext::Issue(_) => {
                    anyhow::bail!("local item #{} is not a PR", locator.number)
                }
            },
        }
    }

    /// Local PR generations start from the frozen candidate, not a mutable source branch.
    pub(crate) fn pr_worktree_ref(&self, number: u64, head_ref: &str) -> Result<String> {
        match self {
            Self::GitHub(_) => Ok(head_ref.to_owned()),
            Self::Local { objects, .. } => objects
                .lock()
                .map_err(|_| anyhow::anyhow!("local object store lock poisoned"))?
                .item(number)?
                .head_commit
                .ok_or_else(|| {
                    anyhow::anyhow!("local PR #{number} lacks a frozen candidate commit")
                }),
        }
    }

    pub(crate) async fn default_branch(&self) -> Result<String> {
        match self {
            Self::GitHub(github) => Ok(github.repository_details().await?.default_branch),
            Self::Local { default_branch, .. } => Ok(default_branch.clone()),
        }
    }

    pub(crate) fn assignee_matches(&self, actor: &Actor, profile_id: &str) -> bool {
        match self {
            Self::GitHub(github) => {
                actor.node_id == github.identity().actor_node_id
                    || actor.login == github.identity().actor_login
            }
            Self::Local { .. } => actor.login == profile_id,
        }
    }
}
