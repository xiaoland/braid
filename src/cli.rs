use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde::Serialize;

use crate::{
    config::Config,
    doctor,
    store::{MigrationPlan, MigrationResult, StoreActor, StoreStatus},
    telemetry,
};

#[derive(Debug, Parser)]
#[command(name = "braid", version, about = "GitHub working memory for local coding agents")]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
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
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
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
        Command::Telemetry { command: TelemetryCommand::Probe(arguments) } => {
            telemetry_probe(arguments).await?;
        }
    }
    Ok(())
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
            profile.context_soft_ratio * 100.0,
            profile.context_hard_bytes
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
    let status = LocalStatus {
        binary_version: env!("CARGO_PKG_VERSION"),
        config_schema: config.schema_version,
        repository: &config.github.repository,
        default_pr_profile: &config.profile_selection.default_pr_profile,
        database,
        telemetry_endpoint: config.telemetry.endpoint.to_string(),
        telemetry_sample_ratio: config.telemetry.effective_sample_ratio(),
        telemetry_incident_mode: config.telemetry.incident_mode,
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
