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
            config_check(&arguments)?;
        }
        Command::Profile { command: ProfileCommand::Inspect(arguments) } => {
            profile_inspect(&arguments)?;
        }
        Command::Migrate { command } => migrate(command)?,
        Command::Status(arguments) => {
            status(&arguments)?;
        }
        Command::Doctor(arguments) => {
            doctor(&arguments).await?;
        }
        Command::Context { command } => Box::pin(context(command)).await?,
        Command::Github { command } => github(command).await?,
        Command::Gh { command } => Box::pin(gh(command)).await?,
        Command::Telemetry { command: TelemetryCommand::Probe(arguments) } => {
            telemetry_probe(arguments).await?;
        }
        Command::Tunnel { command: TunnelCommand::Probe(arguments) } => {
            tunnel_probe(arguments).await?;
        }
        Command::Serve(arguments) => {
            let config = load(&arguments.config)?;
            crate::runtime::serve(config, arguments.tunnel, !arguments.transport_only).await?;
        }
        Command::Setup(arguments) => {
            setup::run(arguments).await?;
        }
    }
    Ok(())
}

async fn gh(command: GhCommand) -> Result<()> {
    match command {
        GhCommand::Comment { command: GhCommentCommand::Create(arguments) } => {
            let config = load(&arguments.config)?;
            let target = arguments.target.parse::<WorkItemLocator>()?;
            let body = match (arguments.body, arguments.body_file) {
                (Some(body), None) => body,
                (None, Some(path)) => fs::read_to_string(&path).with_context(|| {
                    format!("cannot read Agent comment body {}", path.display())
                })?,
                (None, None) => bail!("comment create requires --body or --body-file"),
                (Some(_), Some(_)) => unreachable!("clap enforces body argument conflicts"),
            };
            let actor = store(&config)?;
            let receipt = writer::create_comment(
                &config,
                &actor,
                CommentCreateRequest {
                    target,
                    profile_id: arguments.profile,
                    body,
                    request_id: arguments.request_id,
                },
            )
            .await?;
            print_gh_receipt(&receipt, arguments.json)
        }
        GhCommand::Pr { command: GhPrCommand::Ensure(arguments) } => {
            let config = load(&arguments.config)?;
            let actor = store(&config)?;
            let receipt = writer::ensure_pull_request(
                &config,
                &actor,
                PullRequestEnsureRequest { comment_id: arguments.comment, head: arguments.head },
            )
            .await?;
            if arguments.json {
                print_json(&PullRequestEnsureResult {
                    operation: "pr_ensure",
                    state: &receipt.write.lifecycle,
                    stage: &receipt.stage,
                    implementation_request_comment: receipt.comment_database_id,
                    issue: format!("{}#{}", receipt.write.repository, receipt.issue_number),
                    base: &receipt.base_ref,
                    head: &receipt.head_ref,
                    profile: &receipt.pr_profile_id,
                    pull_request: receipt
                        .pull_request_number
                        .map(|number| format!("{}#{number}", receipt.write.repository)),
                    error: receipt.write.last_error.as_deref(),
                })?;
            } else {
                println!("state: {} / {}", receipt.write.lifecycle, receipt.stage);
                println!("Implementation Request: {}", receipt.write.target);
                println!("head: {}", receipt.head_ref);
                if let Some(number) = receipt.pull_request_number {
                    println!("PR: {}#{number}", receipt.write.repository);
                }
                println!("PR Profile: {}", receipt.pr_profile_id);
            }
            Ok(())
        }
    }
}

fn print_gh_receipt(receipt: &crate::store::GhWriteReceipt, json: bool) -> Result<()> {
    if json {
        print_json(&CommentWriteResult {
            operation: &receipt.operation,
            state: &receipt.lifecycle,
            target: &receipt.target,
            profile: &receipt.profile_id,
            role: &receipt.role,
            comment: receipt
                .remote_database_id
                .as_ref()
                .map(|id| format!("{}#issuecomment-{id}", receipt.repository)),
            error: receipt.last_error.as_deref(),
        })?;
    } else {
        println!("operation: {}", receipt.operation);
        println!("state: {}", receipt.lifecycle);
        println!("target: {}", receipt.target);
        println!("Profile: {} ({})", receipt.profile_id, receipt.role);
        if let Some(id) = &receipt.remote_database_id {
            println!("GitHub comment: {}#issuecomment-{id}", receipt.repository);
        }
        if let Some(error) = &receipt.last_error {
            println!("error: {error}");
        }
    }
    Ok(())
}

async fn tunnel_probe(arguments: TunnelProbe) -> Result<()> {
    let config = load(&arguments.config)?;
    crate::runtime::probe_public_webhook(&config, &arguments.url).await?;
    println!("public webhook probe: accepted");
    Ok(())
}

async fn context(command: ContextCommand) -> Result<()> {
    let (arguments, kind) = match command {
        ContextCommand::Issue(arguments) => (arguments, "issue"),
        ContextCommand::Pr(arguments) => (arguments, "pr"),
    };
    let config = load(&arguments.config)?;
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
    let store = store(&config)?;
    context::reconcile_local_state(&mut canonical, &store)?;
    let rendered = context::render(
        &canonical,
        profile.github_context_soft_ratio,
        profile.github_context_hard_bytes,
    )?;
    context::record_context_revision(&canonical, &rendered, &store)?;
    if arguments.json {
        print_json(&ContextReport {
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

async fn github_probe(arguments: GitHubProbe) -> Result<()> {
    let config = load(&arguments.config)?;
    let repository = arguments.repository.parse::<RepositoryName>()?;
    let client =
        GitHubClient::connect(&config.github, &repository).await.context("GitHub probe failed")?;
    if arguments.json {
        print_json(client.identity())?;
    } else {
        let identity = client.identity();
        println!("GitHub App: {} ({})", identity.app_slug, identity.app_id);
        println!("installation: {}", identity.installation_id);
        println!("repository: {}", identity.repository);
        println!("repository node: {}", identity.repository_node_id);
        println!("App actor: {} ({})", identity.actor_login, identity.actor_node_id);
        println!("token expires: {}", identity.token_expires_at);
        println!(
            "permissions: {}",
            identity
                .permissions
                .iter()
                .map(|(name, level)| format!("{name}={level}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

async fn github(command: GitHubCommand) -> Result<()> {
    match command {
        GitHubCommand::Probe(arguments) => github_probe(arguments).await,
        GitHubCommand::Webhook(arguments) => {
            let config = load(&arguments.config)?;
            let repository = config.github.repository.parse::<RepositoryName>()?;
            let client = GitHubClient::connect(&config.github, &repository).await?;
            let webhook = client.app_webhook_config().await?;
            if arguments.json {
                print_json(&webhook)?;
            } else {
                println!("URL: {}", webhook.url);
                println!("content type: {}", webhook.content_type);
                println!("insecure SSL: {}", webhook.insecure_ssl);
            }
            Ok(())
        }
        GitHubCommand::Deliveries(arguments) => {
            let config = load(&arguments.config)?;
            let repository = config.github.repository.parse::<RepositoryName>()?;
            let client = GitHubClient::connect(&config.github, &repository).await?;
            let deliveries = client.app_deliveries().await?;
            if arguments.json {
                print_json(&deliveries)?;
            } else {
                for delivery in deliveries {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        delivery.id,
                        delivery.guid,
                        delivery.event,
                        delivery.action.as_deref().unwrap_or(""),
                        delivery.status
                    );
                }
            }
            Ok(())
        }
        GitHubCommand::Redeliver(arguments) => {
            let config = load(&arguments.config)?;
            let repository = config.github.repository.parse::<RepositoryName>()?;
            let client = GitHubClient::connect(&config.github, &repository).await?;
            client.redeliver(arguments.delivery_id).await?;
            println!("redelivery requested: {}", arguments.delivery_id);
            Ok(())
        }
    }
}

fn config_check(arguments: &ConfigPath) -> Result<()> {
    let config = load(&arguments.config)?;
    if arguments.json {
        print_json(&config.summary())?;
    } else {
        println!("configuration: valid");
        println!("schema: {}", config.schema_version);
        println!("repository: {}", config.github.repository);
        println!("database: {}", config.runtime.database.display());
        println!(
            "profiles: {}",
            config
                .profiles
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("effective trace sample ratio: {}", config.telemetry.effective_sample_ratio());
    }
    Ok(())
}

fn profile_inspect(arguments: &ProfileInspect) -> Result<()> {
    let config = load(&arguments.config)?;
    let profile = config.profile(&arguments.profile)?;
    if arguments.json {
        print_json(profile)?;
    } else {
        println!("profile: {} ({})", profile.id, profile.display_name);
        println!("tags: {}", profile.tags.join(", "));
        println!("provider: {}", profile.provider);
        println!("model: {}", profile.model.as_deref().unwrap_or("provider default"));
        println!("reasoning: {}", profile.reasoning.as_deref().unwrap_or("provider default"));
        println!("workspace: {}", profile.workspace.display());
        println!("status surfaces: {}", profile.status_surfaces.join(", "));
        println!(
            "context budget: {:.0}% / {} bytes hard",
            profile.github_context_soft_ratio * 100.0,
            profile.github_context_hard_bytes
        );
        println!("user instructions:\n{}", profile.user_instructions);
    }
    Ok(())
}

fn migrate(command: MigrateCommand) -> Result<()> {
    let (arguments, apply) = match command {
        MigrateCommand::Plan(arguments) => (arguments, false),
        MigrateCommand::Apply(arguments) => (arguments, true),
    };
    let config = load(&arguments.config)?;
    let actor = store(&config)?;
    if apply {
        let result = actor.apply()?;
        if arguments.json {
            print_json(&result)?;
        } else {
            print_migration_result(&result);
        }
    } else {
        let plan = actor.plan()?;
        if arguments.json {
            print_json(&plan)?;
        } else {
            print_migration_plan(&plan);
        }
    }
    Ok(())
}

fn status(arguments: &ConfigPath) -> Result<()> {
    let config = load(&arguments.config)?;
    let database = store(&config)?.status()?;
    let transport = if database.pending_migrations == 0 {
        Some(store(&config)?.runtime_status()?)
    } else {
        None
    };
    let status = LocalStatus {
        binary_version: env!("CARGO_PKG_VERSION"),
        config_schema: config.schema_version,
        repository: &config.github.repository,
        default_pr_profile: &config.profile_selection.default_pr_profile,
        database,
        telemetry_endpoint: config.telemetry.endpoint.to_string(),
        telemetry_sample_ratio: config.telemetry.effective_sample_ratio(),
        telemetry_incident_mode: config.telemetry.incident_mode,
        transport,
    };
    if arguments.json {
        print_json(&status)?;
    } else {
        println!("braid {}", status.binary_version);
        println!("repository: {}", status.repository);
        println!(
            "database schema: {}/{} ({} pending)",
            status.database.schema_version,
            status.database.supported_schema,
            status.database.pending_migrations
        );
        println!("database: {}", status.database.database.display());
        println!("OTLP: {} (ratio {})", status.telemetry_endpoint, status.telemetry_sample_ratio);
        if let Some(transport) = &status.transport {
            println!(
                "transport: deliveries={} duplicates={} pending={} runnable={} writes={}/{} resets={}",
                transport.deliveries,
                transport.duplicate_deliveries,
                transport.pending_batches,
                transport.runnable_batches,
                transport.pending_writes,
                transport.uncertain_writes,
                transport.context_resets.len()
            );
        }
    }
    Ok(())
}

async fn doctor(arguments: &ConfigPath) -> Result<()> {
    let config = load(&arguments.config)?;
    let report = doctor::run(&config).await;
    if arguments.json {
        print_json(&report)?;
    } else {
        for check in &report.checks {
            println!("{:?}\t{}\t{}", check.state, check.name, check.detail);
        }
    }
    if !report.ready {
        bail!("doctor found failing or unavailable preconditions");
    }
    Ok(())
}

async fn telemetry_probe(arguments: TelemetryProbe) -> Result<()> {
    let config = load(&arguments.config)?;
    let telemetry_config = config.telemetry;
    let marker = arguments.marker;
    let result =
        tokio::task::spawn_blocking(move || telemetry::run_probe(&telemetry_config, &marker))
            .await
            .context("telemetry worker panicked")??;
    if arguments.json {
        print_json(&result)?;
    } else {
        println!("sampled: {}", result.sampled);
        println!("payload emitted: {}", result.payload_emitted);
        println!("OTLP endpoint: {}", result.exporter.endpoint);
    }
    Ok(())
}

fn load(path: &Path) -> Result<Config> {
    if !path.is_absolute() {
        bail!("--config must be an absolute path: {}", path.display());
    }
    Config::load(path).with_context(|| format!("configuration rejected: {}", path.display()))
}

fn parse_page_size(value: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .ok()
        .filter(|page_size| (1..=100).contains(page_size))
        .ok_or_else(|| "page size must be between 1 and 100".to_owned())
}

fn store(config: &Config) -> Result<StoreActor> {
    StoreActor::start(config.runtime.database.clone(), config.runtime.backups.clone())
        .context("cannot start SQLite actor")
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn print_migration_plan(plan: &MigrationPlan) {
    println!("database: {}", plan.database.display());
    println!("schema: {}/{}", plan.current_schema, plan.supported_schema);
    if plan.pending.is_empty() {
        println!("pending: none");
    } else {
        for migration in &plan.pending {
            println!("pending: {:04} {} {}", migration.version, migration.name, migration.checksum);
        }
    }
}

fn print_migration_result(result: &MigrationResult) {
    println!("schema: {} -> {}", result.previous_schema, result.current_schema);
    println!(
        "applied: {}",
        result.applied.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
    );
    if let Some(backup) = &result.backup {
        println!("backup: {}", backup.display());
    }
}
