use crate::{
    builtin::BuiltinClient,
    config::Config,
    context,
    platform::grants::WriterGrant,
    platform::objects::{CandidateLocation, CommandRequest, ControlReceipt, ObjectCommand},
    store::{BuiltinCommand, BuiltinReply, StoreActor},
};
use anyhow::{Context as _, Result, ensure};
use clap::{Args, Subcommand};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Debug, Subcommand)]
pub enum LocalCommand {
    /// Create a Braid-owned Issue.
    Issue(CreateIssue),
    /// Create a PR from two existing Git revisions.
    Pr(CreatePr),
    List,
    Show(Target),
    Context(Target),
    /// Read a command receipt without resubmitting its payload.
    Receipt(ReceiptTarget),
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
pub struct ReceiptTarget {
    pub request_id: String,
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

#[allow(clippy::too_many_lines)]
pub async fn run_options(
    command: LocalCommand,
    config: &Config,
    request_id: Option<String>,
    expected_revision: Option<u64>,
) -> Result<()> {
    let store = Arc::new(StoreActor::start(
        config.runtime.database().to_path_buf(),
        config.runtime.backups().to_path_buf(),
    )?);
    let client = BuiltinClient::connect(config, Arc::clone(&store)).await?;
    if let LocalCommand::Receipt(target) = &command {
        let mut url = config
            .builtin
            .as_ref()
            .context("built-in upstream is required")?
            .endpoint
            .join("api/control/receipts/")?;
        url.path_segments_mut()
            .map_err(|()| anyhow::anyhow!("invalid platform URL"))?
            .pop_if_empty()
            .push(&target.request_id);
        let result: ControlReceipt = client.get(url.as_str()).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    let writer = if std::env::var_os("BRAID_AGENT").is_some() {
        Some(active_writer(config.runtime.database(), &std::env::current_dir()?)?)
    } else {
        None
    };
    let actor = writer.as_ref().map_or("human", |writer| writer.actor.as_str()).to_owned();
    let capability = if let Some(writer) = &writer {
        let BuiltinReply::Grant(Some(grant)) =
            store.builtin(BuiltinCommand::Grant(writer.session_id.clone()))?
        else {
            anyhow::bail!(
                "Agent has no active platform writer capability; refusing operator fallback"
            )
        };
        ensure!(
            grant.lifecycle == "active"
                && grant.actor == writer.actor
                && grant.item_number == writer.number,
            "Agent writer capability does not match current assignment"
        );
        Some(WriterGrant {
            grant_id: grant.grant_id.context("writer grant has no identity")?,
            epoch: grant.epoch,
            token: grant.token.context("writer grant has no capability")?,
        })
    } else {
        None
    };
    let object = match command {
        LocalCommand::Receipt(_) => unreachable!("receipt handled before writer admission"),
        LocalCommand::List => {
            let result: serde_json::Value = client.get("api/control/items").await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        LocalCommand::Show(a) => {
            let result: serde_json::Value =
                client.get(&format!("api/control/items/{}", a.number)).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        LocalCommand::Context(a) => {
            let canonical = client.context(a.number).await?;
            print!("{}", context::render_complete(&canonical, 1.0, usize::MAX).text);
            return Ok(());
        }
        LocalCommand::Diff(a) => {
            let result: serde_json::Value =
                client.get(&format!("api/control/items/{}/diff", a.number)).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            return Ok(());
        }
        LocalCommand::Issue(a) => {
            ObjectCommand::CreateIssue { title: a.title, body: a.body.read()?.unwrap_or_default() }
        }
        LocalCommand::Pr(a) => ObjectCommand::CreatePr {
            title: a.title,
            body: a.body.read()?.unwrap_or_default(),
            head: a.head,
            base: a.base,
            issue: a.issue,
            issue_revision: if let Some(issue) = a.issue {
                Some(client.item(issue).await?.revision)
            } else {
                None
            },
        },
        LocalCommand::Comment(a) => ObjectCommand::Comment {
            number: a.number,
            body: a.body.read()?.context("provide --body or --body-file")?,
            reply_to: a.reply_to,
        },
        LocalCommand::Edit(a) => {
            ObjectCommand::Edit { number: a.number, title: a.title, body: a.body.read()? }
        }
        LocalCommand::Assign(a) => ObjectCommand::Assign { number: a.number, actor: a.profile },
        LocalCommand::Unassign(a) => ObjectCommand::Unassign { number: a.number },
        LocalCommand::Close(a) => ObjectCommand::Close { number: a.number },
        LocalCommand::Reopen(a) => ObjectCommand::Reopen { number: a.number },
        LocalCommand::Link(a) => ObjectCommand::Link {
            issue: a.issue,
            pr: a.pr,
            expected_pr_revision: client.item(a.pr).await?.revision,
        },
        LocalCommand::Merge(a) => ObjectCommand::Merge { number: a.number },
        LocalCommand::Publish(a) => {
            let cwd = std::env::current_dir()?.canonicalize()?;
            let source = writer.as_ref().map_or_else(
                || {
                    config
                        .profiles
                        .first()
                        .map(crate::config::Profile::workspace)
                        .context("no Harness workspace configured")
                },
                |writer| Ok(writer.worktree.as_path()),
            )?;
            ObjectCommand::Publish {
                number: a.number,
                candidate: CandidateLocation::Commit { commit: published_commit(source, &cwd)? },
            }
        }
    };
    let revision = if let Some(revision) = expected_revision {
        Some(revision)
    } else if let Some(number) = object.target() {
        Some(client.item(number).await?.revision)
    } else {
        None
    };
    let request_id = request_id.unwrap_or_else(|| uuid::Uuid::now_v7().to_string());
    let request = CommandRequest {
        request_id: request_id.clone(),
        expected_revision: revision,
        command: object,
        actor,
        writer: capability,
    };
    let result: ControlReceipt = client
        .post("api/control/commands", &request)
        .await
        .with_context(|| format!("platform command requestId={request_id}"))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    ensure!(
        result.state == "applied",
        "platform command requestId={request_id} settled as {}: {}",
        result.state,
        result.error.as_ref().map(ToString::to_string).unwrap_or_default()
    );
    Ok(())
}

struct AgentWriter {
    session_id: String,
    actor: String,
    number: u64,
    worktree: PathBuf,
}
fn active_writer(database: &Path, cwd: &Path) -> Result<AgentWriter> {
    let cwd = cwd.canonicalize()?;
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut query=connection.prepare("SELECT s.provider_session_id,g.profile_id,i.number,w.path FROM provider_sessions s JOIN turns t ON t.session_id=s.session_id JOIN agent_instances g ON g.agent_id=s.agent_id JOIN assignments a ON a.assignment_id=g.assignment_id JOIN work_items i ON i.node_id=a.work_item_node_id JOIN worktrees w ON w.agent_id=g.agent_id WHERE s.lifecycle='running' AND t.lifecycle IN ('starting','running') AND a.lifecycle IN ('active','finalizing') AND g.lifecycle IN ('idle','finalizing') AND w.lifecycle='active'")?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut matches = Vec::new();
    for (session_id, actor, number, path) in rows {
        let worktree = PathBuf::from(path).canonicalize()?;
        if cwd.starts_with(&worktree) {
            matches.push(AgentWriter {
                session_id,
                actor,
                number: u64::try_from(number)?,
                worktree,
            });
        }
    }
    ensure!(
        matches.len() == 1,
        "Agent caller has no unique active assignment/session/turn for this worktree"
    );
    Ok(matches.pop().expect("exactly one active caller"))
}
fn published_commit(source: &Path, cwd: &Path) -> Result<String> {
    let expected = git2::Repository::open(source)?;
    let actual = git2::Repository::discover(cwd)?;
    ensure!(
        expected.commondir().canonicalize()? == actual.commondir().canonicalize()?,
        "candidate is not in the Harness source Git object store"
    );
    let workdir =
        actual.workdir().context("candidate must be a non-bare worktree")?.canonicalize()?;
    ensure!(cwd.starts_with(workdir), "candidate cwd is outside its Git worktree");
    ensure!(actual.statuses(None)?.is_empty(), "publish requires a clean committed worktree");
    Ok(actual.head()?.peel_to_commit()?.id().to_string())
}
