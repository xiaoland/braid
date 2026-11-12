#![allow(clippy::wildcard_imports)]
use super::*;
use crate::github::BodySnapshot;
use helpers::{RemoteWrite, WriteFailure, WriteResult};

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ReadForEdit {
    target: String,
    snapshot: BodySnapshot,
    digest: String,
}

pub struct EditRequest {
    pub invocation: Invocation,
    pub target: WorkItemLocator,
    pub profile_id: String,
    pub snapshot: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub request_id: String,
}

pub async fn read_for_edit(config: &Config, target: &WorkItemLocator) -> Result<String> {
    helpers::require_configured_repository(config, target)?;
    let github = GitHubClient::connect(config.github()?, &target.repository).await?;
    read_snapshot(&github, target).await
}

pub(super) async fn read_snapshot(
    github: &GitHubClient,
    target: &WorkItemLocator,
) -> Result<String> {
    let snapshot = github.body_snapshot(target.number).await?;
    Ok(serde_json::to_string_pretty(&ReadForEdit {
        target: target.to_string(),
        digest: snapshot.digest(),
        snapshot,
    })?)
}

pub fn edit(config: &Config, store: &StoreActor, request: EditRequest) -> Result<GhWriteReceipt> {
    helpers::require_configured_repository(config, &request.target)?;
    config.github()?;
    helpers::validate_request_id(&request.request_id)?;
    let profile = config.profile(&request.profile_id)?;
    if config.profile_repository(profile) != request.target.repository.to_string() {
        bail!("Profile repository mismatch");
    }
    let read: ReadForEdit = serde_json::from_str(&request.snapshot)?;
    if read.target != request.target.to_string() || read.digest != read.snapshot.digest() {
        bail!("snapshot target or digest mismatch; read the canonical target again");
    }
    if request.title.is_none() && request.body.is_none() {
        bail!("edit requires --title or --body-file");
    }
    let source = resolve_source(config, Some(&profile.id), &request.invocation)?;
    let role = helpers::publication_role(profile, source.as_ref(), read.snapshot.kind())?;
    let title = request.title.unwrap_or(read.snapshot.title);
    if title.trim().is_empty() || title.len() > 256 {
        bail!("title must contain 1-256 bytes");
    }
    let old_body = read.snapshot.body.unwrap_or_default();
    let body = match request.body {
        Some(body) => render_maintained_body(profile, role, &body, &old_body)?,
        None => old_body,
    };
    if body.len() > COMMENT_BODY_LIMIT {
        bail!("body exceeds safe byte limit");
    }
    let payload = serde_json::to_string(&AcceptedRequest::Edit {
        profile: profile.clone(),
        expected: read.digest,
        title,
        body,
    })?;
    store
        .prepare_gh_write(NewGhWriteIntent {
            request_key: format!(
                "body_edit\0{}\0{}\0{}",
                request.target,
                source.as_ref().map_or_else(
                    || "operator".into(),
                    |s| format!("{}:{}", s.assignment_id, s.generation)
                ),
                request.request_id
            ),
            operation: "body_edit",
            repository: request.target.repository.to_string(),
            target: request.target.to_string(),
            profile_id: profile.id.clone(),
            role: role.into(),
            source,
            request_digest: helpers::digest(&payload),
            payload,
        })
        .map_err(Into::into)
}

fn render_maintained_body(
    profile: &Profile,
    role: &str,
    body: &str,
    old_body: &str,
) -> Result<String> {
    let mut body = body.trim().to_owned();
    if body.trim().is_empty() {
        bail!("body must not be empty");
    }
    // Attribution is owned by the source, not copied from a previous writer.
    while body.starts_with("> **Braid Agent ·") {
        body = body.split_once("\n\n").map_or("", |(_, rest)| rest).to_owned();
    }
    let protected = protected_body_sections(old_body);
    let prefix = protected.join("\n\n");
    while !prefix.is_empty() && body.starts_with(&prefix) {
        body = body[prefix.len()..].trim_start().to_owned();
    }
    // Prefix complete original Markdown paragraphs so an unclosed code fence or
    // HTML comment in the replacement cannot hide a closing reference/marker.
    if !protected.is_empty() {
        body = format!("{prefix}\n\n{body}");
    }
    helpers::render_agent_comment(profile, role, &body)
}

fn protected_body_sections(old_body: &str) -> Vec<String> {
    use comrak::{Arena, Options, nodes::NodeValue, parse_document};
    let arena = Arena::new();
    let root = parse_document(&arena, old_body, &Options::default());
    let lines: Vec<_> = old_body.lines().collect();
    root.descendants()
        .filter_map(|node| {
            let data = node.data.borrow();
            if !matches!(data.value, NodeValue::Paragraph) {
                return None;
            }
            let section = lines
                .get(data.sourcepos.start.line.saturating_sub(1)..data.sourcepos.end.line)?
                .join("\n");
            let lower = section.to_ascii_lowercase();
            let closing = (section.contains('#') || section.contains("/issues/"))
                && [
                    "close", "closes", "closed", "fix", "fixes", "fixed", "resolve", "resolves",
                    "resolved",
                ]
                .iter()
                .any(|word| {
                    lower.split(|c: char| !c.is_ascii_alphabetic()).any(|token| token == *word)
                });
            (closing || section.contains("Implementation request:")).then_some(section)
        })
        .collect()
}

/// Operation permission is independent of Profile selection tags.
pub(super) async fn authorize(
    config: &Config,
    github: &GitHubClient,
    receipt: &GhWriteReceipt,
    editing: bool,
) -> Result<(), GitHubError> {
    let target: WorkItemLocator = receipt.target.parse()?;
    let item = github.issue_or_pull_request(target.number).await?;
    if editing && !item.state.eq_ignore_ascii_case("open") {
        return Err(GitHubError::GraphQl("body target is closed".into()));
    }
    let Some(source) = &receipt.source else {
        return Ok(());
    };
    let (repository, kind, number) =
        crate::store::publication_source::work_item(config.runtime.database(), source)
            .map_err(|e| GitHubError::GraphQl(e.to_string()))?;
    if repository != target.repository.to_string() {
        return Err(GitHubError::GraphQl("source repository mismatch".into()));
    }
    if number == target.number && kind == item.kind() {
        return Ok(());
    }
    let related = if kind == "pr" && item.kind() == "issue" {
        github
            .pull_request_closing_issues(number)
            .await?
            .iter()
            .any(|i| i.repository == repository && i.number == target.number)
    } else if !editing && kind == "issue" && item.kind() == "pr" {
        github
            .pull_request_closing_issues(target.number)
            .await?
            .iter()
            .any(|i| i.repository == repository && i.number == number)
    } else {
        false
    };
    if !related {
        return Err(GitHubError::GraphQl(
            "target is not a permitted directly associated work item".into(),
        ));
    }
    if editing {
        let source_item = github.issue_or_pull_request(number).await?;
        if !source_item.state.eq_ignore_ascii_case("open") {
            return Err(GitHubError::GraphQl("source PR is closed".into()));
        }
    }
    Ok(())
}

pub(super) async fn execute(
    config: &Config,
    github: &GitHubClient,
    receipt: &GhWriteReceipt,
    mutation_possible: &mut bool,
) -> WriteResult {
    let AcceptedRequest::Edit { expected, title, body, .. } =
        serde_json::from_str(&receipt.payload).map_err(|e| GitHubError::GraphQl(e.to_string()))?
    else {
        return Err(GitHubError::GraphQl("invalid body edit payload".into()).into());
    };
    let target: WorkItemLocator = receipt.target.parse()?;
    let current = github.body_snapshot(target.number).await?;
    // Recovery checks exact resulting content before any possible retry. A later Human
    // change is a conflict, not evidence of our success and never overwritten.
    if receipt.attempts > 0 && current.title == title && current.body.as_deref() == Some(&body) {
        return Ok(RemoteWrite { database_id: current.id.to_string(), node_id: current.node_id });
    }
    if current.digest() != expected {
        return Err(WriteFailure::GitHub(GitHubError::Http {
            status: axum::http::StatusCode::CONFLICT,
            body: "canonical title/body changed since snapshot; read again".into(),
        }));
    }
    authorize(config, github, receipt, true).await?;
    github.require_write_permissions(&[if current.kind() == "pr" {
        "pull_requests"
    } else {
        "issues"
    }])?;
    let latest = github.body_snapshot(target.number).await?;
    if latest.digest() != expected {
        return Err(GitHubError::Http {
            status: axum::http::StatusCode::CONFLICT,
            body: "canonical version changed during validation; read again".into(),
        }
        .into());
    }
    check_source(config, receipt)?;
    *mutation_possible = true;
    let result = github.edit_body(target.number, &title, &body).await?;
    if result.title != title || result.body.as_deref() != Some(&body) {
        return Err(GitHubError::ConvergencePending(
            "GitHub did not confirm requested title/body".into(),
        )
        .into());
    }
    Ok(RemoteWrite { database_id: result.id.to_string(), node_id: result.node_id })
}
