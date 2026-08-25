#![allow(clippy::large_futures)]
use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{
    config::Config,
    context::{self, CanonicalContext, ContextPressure},
    doctor,
    github::{GitHubClient, RepositoryName, WorkItemLocator},
    setup,
    store::{MigrationPlan, MigrationResult, StoreActor, StoreStatus},
    telemetry,
    writer::{self, CommentCreateRequest, PullRequestEnsureRequest},
};

mod config;
mod context_cmd;
mod doctor_cmd;
mod gh;
mod gh_cmd;
mod helpers;
mod migrate;
mod profile;
mod status;
mod telemetry_cmd;

#[derive(Debug, Parser)]
#[command(name = "braid", version, about = "GitHub working memory for local coding agents")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Setup(SetupArguments),
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    Status(ConfigPath),
    Doctor(ConfigPath),
    Context {
        #[command(subcommand)]
        command: ContextCommand,
    },
    Github {
        #[command(subcommand)]
        command: GitHubCommand,
    },
    Gh {
        #[command(subcommand)]
        command: GhCommand,
    },
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
    Tunnel {
        #[command(subcommand)]
        command: TunnelCommand,
    },
    Serve(ServeArguments),
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Check(ConfigPath),
}

#[derive(Debug, Subcommand)]
enum ProfileCommand {
    Inspect(ProfileInspect),
}

#[derive(Debug, Subcommand)]
enum MigrateCommand {
    Plan(ConfigPath),
    Apply(ConfigPath),
}

#[derive(Debug, Subcommand)]
enum TelemetryCommand {
    Probe(TelemetryProbe),
}

#[derive(Debug, Subcommand)]
enum TunnelCommand {
    Probe(TunnelProbe),
}

#[derive(Debug, Subcommand)]
enum ContextCommand {
    Issue(ContextArguments),
    Pr(ContextArguments),
}

#[derive(Debug, Subcommand)]
enum GitHubCommand {
    Probe(GitHubProbe),
    Webhook(ConfigPath),
    Deliveries(ConfigPath),
    Redeliver(GitHubRedeliver),
}

#[derive(Debug, Subcommand)]
enum GhCommand {
    Comment {
        #[command(subcommand)]
        command: GhCommentCommand,
    },
    Pr {
        #[command(subcommand)]
        command: GhPrCommand,
    },
}

#[derive(Debug, Subcommand)]
enum GhCommentCommand {
    Create(GhCommentCreate),
}

#[derive(Debug, Subcommand)]
enum GhPrCommand {
    Ensure(GhPrEnsure),
}

#[derive(Debug, Args)]
struct ConfigPath {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct ProfileInspect {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long)]
    profile: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct TelemetryProbe {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long, default_value = "BRAID_OTEL_FULL_PAYLOAD_PROBE")]
    marker: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct TunnelProbe {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long, value_name = "HTTPS_URL")]
    url: String,
}

#[derive(Debug, Args)]
struct ContextArguments {
    #[arg(value_name = "OWNER/REPOSITORY#NUMBER")]
    target: String,
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    json: bool,
    /// GraphQL connection page size. Lower values are useful for live pagination diagnostics.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 100,
        value_parser = parse_page_size
    )]
    page_size: usize,
}

#[derive(Debug, Args)]
struct GitHubProbe {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long, value_name = "OWNER/REPOSITORY")]
    repository: String,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct GitHubRedeliver {
    #[arg(value_name = "DELIVERY_ID")]
    delivery_id: u64,
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
}

#[derive(Debug, Args)]
struct GhCommentCreate {
    #[arg(value_name = "OWNER/REPOSITORY#NUMBER")]
    target: String,
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long)]
    profile: String,
    #[arg(long, conflicts_with = "body_file")]
    body: Option<String>,
    #[arg(long, value_name = "PATH", conflicts_with = "body")]
    body_file: Option<PathBuf>,
    #[arg(long, value_name = "TOKEN")]
    request_id: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct GhPrEnsure {
    #[arg(long, value_name = "ISSUE_COMMENT_ID")]
    comment: u64,
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(long, value_name = "BRANCH")]
    head: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SetupArguments {
    #[arg(value_name = "OWNER/REPOSITORY")]
    pub(crate) repository: String,
    #[arg(long, value_name = "pi|codex", default_value = "pi")]
    pub(crate) provider: String,
    #[arg(long, value_name = "MODEL", default_value = "deepseek-chat")]
    pub(crate) model: String,
    #[arg(long, value_name = "ENV", default_value = "DEEPSEEK_API_KEY")]
    pub(crate) api_key_environment: String,
    #[arg(long, value_name = "DIR", default_value = "~/.braid")]
    pub(crate) home: PathBuf,
    #[arg(
        long,
        help = "Skip opening a browser; print manifest values and manual instructions instead"
    )]
    pub(crate) no_browser: bool,
}

#[derive(Debug, Args)]
struct ServeArguments {
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    #[arg(
        long,
        help = "Expose ingress through a free Wrangler Quick Tunnel and own the App webhook while running"
    )]
    tunnel: bool,
    #[arg(long, help = "Run GitHub transport without starting the configured Coding Agent")]
    transport_only: bool,
}

#[derive(Debug, Serialize)]
struct ContextReport<'a> {
    target: &'a WorkItemLocator,
    profile: &'a str,
    bytes: usize,
    pressure: ContextPressure,
}

#[derive(Debug, Serialize)]
struct CommentWriteResult<'a> {
    operation: &'a str,
    state: &'a str,
    target: &'a str,
    profile: &'a str,
    role: &'a str,
    comment: Option<String>,
    error: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct PullRequestEnsureResult<'a> {
    operation: &'static str,
    state: &'a str,
    stage: &'a str,
    implementation_request_comment: u64,
    issue: String,
    base: &'a str,
    head: &'a str,
    profile: &'a str,
    pull_request: Option<String>,
    error: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct LocalStatus<'a> {
    binary_version: &'static str,
    config_schema: u32,
    repository: &'a str,
    default_pr_profile: &'a str,
    database: StoreStatus,
    telemetry_endpoint: String,
    telemetry_sample_ratio: f64,
    telemetry_incident_mode: bool,
    transport: Option<crate::store::RuntimeStoreStatus>,
}

pub async fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Config { command: ConfigCommand::Check(arguments) } => {
            config::config_check(&arguments)?;
        }
        Command::Profile { command: ProfileCommand::Inspect(arguments) } => {
            profile::profile_inspect(&arguments)?;
        }
        Command::Migrate { command } => migrate::migrate(command)?,
        Command::Status(arguments) => {
            status::status(&arguments)?;
        }
        Command::Doctor(arguments) => {
            doctor_cmd::doctor(&arguments).await?;
        }
        Command::Context { command } => Box::pin(context_cmd::context(command)).await?,
        Command::Github { command } => gh::github(command).await?,
        Command::Gh { command } => Box::pin(gh_cmd::gh(command)).await?,
        Command::Telemetry { command: TelemetryCommand::Probe(arguments) } => {
            telemetry_cmd::telemetry_probe(arguments).await?;
        }
        Command::Tunnel { command: TunnelCommand::Probe(arguments) } => {
            helpers::tunnel_probe(arguments).await?;
        }
        Command::Serve(arguments) => {
            let config = helpers::load(&arguments.config)?;
            crate::runtime::serve(config, arguments.tunnel, !arguments.transport_only).await?;
        }
        Command::Setup(arguments) => {
            setup::run(arguments).await?;
        }
    }
    Ok(())
}

fn parse_page_size(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|page_size| (1..=100).contains(page_size))
        .ok_or_else(|| "page size must be between 1 and 100".to_owned())
}
