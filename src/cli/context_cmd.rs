#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn context(command: ContextCommand) -> Result<()> {
    let (arguments, kind) = match command {
        ContextCommand::Issue(arguments) => (arguments, "issue"),
        ContextCommand::Pr(arguments) => (arguments, "pr"),
    };
    let config_path = arguments.source.resolve_config_path()?;
    let config = helpers::load(&config_path)?;
    let locator = arguments.target.parse::<WorkItemLocator>()?;
    let profile = match arguments.profile.as_deref() {
        Some(profile) => config.profile(profile)?,
        None if kind == "pr" => config.profile(&config.profile_selection.default_pr_profile)?,
        None => config
            .profiles
            .iter()
            .find(|profile| profile.has_tag("issue"))
            .context("configuration has no Profile tagged issue")?,
    };
    if !profile.has_tag(kind) {
        bail!("Profile {:?} is not tagged {kind}", profile.id);
    }
    let client = GitHubClient::connect(&config.github, &locator.repository)
        .await
        .context("GitHub Context is unavailable")?;
    let mut canonical = if kind == "issue" {
        CanonicalContext::Issue(
            context::materialize_issue(&client, &locator, arguments.page_size).await?,
        )
    } else {
        CanonicalContext::PullRequest(
            context::materialize_pull_request(&client, &locator, arguments.page_size).await?,
        )
    };
    let store = helpers::store(&config)?;
    context::reconcile_local_state(&mut canonical, &store)?;
    let rendered = context::render(
        &canonical,
        profile.github_context_soft_ratio,
        profile.github_context_hard_bytes,
    )?;
    context::record_context_revision(&canonical, &rendered, &store)?;
    if arguments.json {
        helpers::print_json(&ContextReport {
            target: &locator,
            profile: &profile.id,
            bytes: rendered.bytes,
            pressure: rendered.pressure,
        })?;
    } else {
        let mut stdout = io::stdout().lock();
        stdout.write_all(rendered.text.as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}
