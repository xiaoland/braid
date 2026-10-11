use crate::{config::Config, context, local::objects::LocalObjects};
use anyhow::{Context as _, Result, ensure};
use clap::{Args, Subcommand};
use std::{fs, path::PathBuf};

#[derive(Debug, Subcommand)]
pub enum LocalCommand {
    /// Create a Braid-owned Issue.
    Issue(CreateIssue),
    /// Create a PR from two existing Git revisions.
    Pr(CreatePr),
    List,
    Show(Target),
    Context(Target),
    Comment(CommentArguments),
    Edit(EditArguments),
    Assign(AssignmentArguments),
    Unassign(Target),
    Close(Target),
    Reopen(Target),
    Link(LinkArguments),
    Diff(Target),
    Merge(Target),
    Publish(Target),
}

#[derive(Debug, Args)]
pub struct Target {
    pub number: u64,
}
#[derive(Debug, Args)]
pub struct Body {
    #[arg(long, conflicts_with = "body_file")]
    pub body: Option<String>,
    #[arg(long, conflicts_with = "body")]
    pub body_file: Option<PathBuf>,
}
impl Body {
    fn read(&self) -> Result<Option<String>> {
        if let Some(path) = &self.body_file {
            return fs::read_to_string(path)
                .with_context(|| format!("read body file {}", path.display()))
                .map(Some);
        }
        Ok(self.body.clone())
    }
}
#[derive(Debug, Args)]
pub struct CreateIssue {
    #[arg(long)]
    pub title: String,
    #[command(flatten)]
    pub body: Body,
}
#[derive(Debug, Args)]
pub struct CreatePr {
    #[arg(long)]
    pub title: String,
    #[command(flatten)]
    pub body: Body,
    #[arg(long)]
    pub head: String,
    #[arg(long)]
    pub base: String,
    #[arg(long)]
    pub issue: Option<u64>,
}
#[derive(Debug, Args)]
pub struct CommentArguments {
    #[arg(long)]
    pub reply_to: Option<u64>,
    pub number: u64,
    #[command(flatten)]
    pub body: Body,
}
#[derive(Debug, Args)]
pub struct EditArguments {
    pub number: u64,
    #[arg(long)]
    pub title: Option<String>,
    #[command(flatten)]
    pub body: Body,
}
#[derive(Debug, Args)]
pub struct AssignmentArguments {
    pub number: u64,
    #[arg(long)]
    pub profile: String,
}
#[derive(Debug, Args)]
pub struct LinkArguments {
    pub issue: u64,
    pub pr: u64,
}

pub fn run(command: LocalCommand, config: &Config) -> Result<()> {
    let local =
        config.local.as_ref().context("braid local requires a local collaboration config")?;
    let mut objects = LocalObjects::open(config.runtime.database(), &local.repository)?;
    objects.bind_source(&local.source)?;
    let caller = if std::env::var_os("BRAID_AGENT").is_some() {
        objects.bind_agent(&std::env::current_dir()?)?
    } else {
        "human".into()
    };
    let author = caller.as_str();
    let result = match command {
        LocalCommand::Issue(a) => serde_json::to_value(objects.create_issue(
            &a.title,
            &a.body.read()?.unwrap_or_default(),
            author,
        )?)?,
        LocalCommand::Pr(a) => serde_json::to_value(objects.create_pr(
            &a.title,
            &a.body.read()?.unwrap_or_default(),
            author,
            &local.source,
            &a.head,
            &a.base,
            a.issue,
        )?)?,
        LocalCommand::List => serde_json::to_value(objects.list()?)?,
        LocalCommand::Show(a) => {
            serde_json::json!({"item":objects.item(a.number)?,"comments":objects.comments(a.number)?,"merge":objects.merge_status(a.number)?})
        }
        LocalCommand::Context(a) => {
            let context = objects.context(a.number)?;
            let rendered = context::render_complete(&context, 1.0, usize::MAX);
            print!("{}", rendered.text);
            return Ok(());
        }
        LocalCommand::Comment(a) => serde_json::to_value(objects.reply(
            a.number,
            &a.body.read()?.context("provide --body or --body-file")?,
            author,
            a.reply_to,
        )?)?,
        LocalCommand::Edit(a) => serde_json::to_value(objects.edit(
            a.number,
            a.title.as_deref(),
            a.body.read()?.as_deref(),
            author,
        )?)?,
        LocalCommand::Assign(a) => {
            config.profile(&a.profile)?;
            let item = objects.item(a.number)?;
            let expected = if item.kind == "pr" {
                &config.profile_selection.default_pr_profile
            } else {
                &config
                    .profiles
                    .iter()
                    .find(|p| p.tags.iter().any(|t| t == "issue"))
                    .context("no Issue profile configured")?
                    .id
            };
            ensure!(
                &a.profile == expected,
                "this runtime supports profile {expected} for {} objects",
                item.kind
            );
            serde_json::to_value(objects.assign(a.number, Some(&a.profile), author)?)?
        }
        LocalCommand::Unassign(a) => serde_json::to_value(objects.assign(a.number, None, author)?)?,
        LocalCommand::Close(a) => {
            serde_json::to_value(objects.set_state(a.number, false, author)?)?
        }
        LocalCommand::Reopen(a) => {
            serde_json::to_value(objects.set_state(a.number, true, author)?)?
        }
        LocalCommand::Link(a) => {
            objects.link(a.issue, a.pr, author)?;
            serde_json::json!({"issue":a.issue,"pr":a.pr})
        }
        LocalCommand::Diff(a) => {
            print!("{}", objects.diff(a.number, &local.source)?);
            return Ok(());
        }
        LocalCommand::Publish(a) => serde_json::to_value(objects.publish(
            a.number,
            &local.source,
            &std::env::current_dir()?,
            author,
        )?)?,
        LocalCommand::Merge(a) => {
            serde_json::to_value(objects.merge(a.number, &local.source, author)?)?
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
