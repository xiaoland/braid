//! Declare a Harness connected to an existing built-in collaboration platform.
use crate::config::{
    BuiltinConfig, CONFIG_SCHEMA_VERSION, Config, InstanceConfig, LlmProvider, LogFormat, Profile,
    ProfileSelection, RuntimeConfig, SchedulerConfig, ServerConfig, TelemetryConfig, ToolConfig,
};
use anyhow::{Context as _, Result, ensure};
use clap::Args;
use std::{fs::OpenOptions, io::Write as _, path::PathBuf};

#[derive(Debug, Args)]
pub(crate) struct InitArguments {
    #[arg(long)]
    pub source: PathBuf,
    #[arg(long)]
    pub config: PathBuf,
    #[arg(long)]
    pub name: String,
    /// Built-in collaboration platform HTTP origin.
    #[arg(long, default_value = "http://127.0.0.1:18082")]
    pub endpoint: url::Url,
    #[arg(long, value_parser=["pi","codex"])]
    pub adapter: String,
    #[arg(long)]
    pub executable: PathBuf,
    /// Pi-only LLM provider catalog id; Braid does not infer one from the adapter.
    #[arg(long, value_name = "ID", required_if_eq("adapter", "pi"))]
    pub llm_provider: Option<String>,
    #[arg(long)]
    pub model: String,
    /// Optional reasoning/thinking level for runtimes that support one.
    #[arg(long, value_name = "LEVEL")]
    pub reasoning: Option<String>,
    /// Pi-only credential kind for the selected LLM provider.
    #[arg(
        long,
        value_name = "api_key|oauth|none",
        value_parser = ["api_key", "oauth", "none"],
        required_if_eq("adapter", "pi")
    )]
    pub llm_credential_kind: Option<String>,
    /// Pi-only Braid-schema credential file; required unless kind is `none`.
    #[arg(long, value_name = "PATH")]
    pub llm_credential_file: Option<PathBuf>,
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run(a: InitArguments) -> Result<()> {
    let source = a.source.canonicalize().context("cannot resolve local Git source")?;
    let repo = git2::Repository::open(&source).context("local source must be a Git checkout")?;
    ensure!(!repo.is_bare(), "local source must have a working tree");
    ensure!(!repo.head_detached()?, "local source needs a named branch");
    ensure!(!a.config.exists(), "config already exists: {}", a.config.display());
    let config_path =
        if a.config.is_absolute() { a.config } else { std::env::current_dir()?.join(a.config) };
    let directory = config_path.parent().context("config path has no parent")?;
    let executable = a.executable.canonicalize().context("cannot resolve Agent executable")?;
    let llm_provider = if a.adapter == "pi" {
        let id = a.llm_provider.clone().context(
            "Pi init requires --llm-provider; Braid does not infer one from the adapter",
        )?;
        let kind = a
            .llm_credential_kind
            .as_deref()
            .context("Pi init requires --llm-credential-kind api_key|oauth|none")
            .and_then(crate::setup::parse_credential_kind)?;
        let credential =
            crate::setup::resolve_credential_arguments(kind, a.llm_credential_file.as_deref())?;
        Some(LlmProvider {
            id,
            credential: Some(credential),
            protocol: None,
            api_key_file: None,
            models: vec![],
            allowances: vec![],
        })
    } else {
        ensure!(
            a.llm_provider.is_none()
                && a.llm_credential_kind.is_none()
                && a.llm_credential_file.is_none(),
            "--llm-provider and --llm-credential-* are only supported for the Pi adapter"
        );
        None
    };
    let profile_provider = llm_provider.as_ref().map(|provider| provider.id.clone());
    let mut runtime = crate::setup::discovery::from_executable(&a.adapter, &executable).await?;
    runtime.home = Some(directory.join("provider").join(&a.adapter));
    let profile = Profile {
        id: "default".into(),
        display_name: "Braid Agent".into(),
        tags: vec!["issue".into(), "pr".into()],
        adapter_type: a.adapter.clone(),
        adapter_version: runtime.version.clone(),
        provider: profile_provider,
        repository: None,
        model: Some(a.model.clone()),
        reasoning: a.reasoning.clone(),
        user_instructions:
            "维护当前需求与决定，交付可采用的实现候选；只在有行动价值时发布简洁评论。".into(),
        skills: Vec::new(),
        native_template: None,
        workspace: Some(source.clone()),
        github_actor_node_id: None,
        status_surfaces: vec!["issue".into(), "pr".into()],
        github_context_soft_ratio: 0.8,
        github_context_hard_bytes: 100_000,
    };
    let mut config = Config {
        finite_run: None,
        schema_version: CONFIG_SCHEMA_VERSION,
        instance: InstanceConfig { key: a.name.clone() },
        local: None,
        builtin: Some(BuiltinConfig { repository: a.name, endpoint: a.endpoint }),
        github: None,
        loaded_from: config_path.clone(),
        runtime: RuntimeConfig {
            root: None,
            database: None,
            backups: None,
            worktrees: None,
            auto_migrate: false,
        },
        scheduler: SchedulerConfig {
            quiet_seconds: 1,
            event_threshold: 8,
            reconciliation_seconds: 60,
        },
        runtimes: vec![runtime],
        llm_providers: llm_provider.into_iter().collect(),
        tools: ToolConfig {
            git: which::which("git").context("Git executable unavailable")?,
            gh: "gh".into(),
            wrangler: "wrangler".into(),
        },
        server: ServerConfig {
            ingress: "127.0.0.1:18080".parse()?,
            health: "127.0.0.1:18081".parse()?,
        },
        telemetry: TelemetryConfig {
            endpoint: "http://127.0.0.1:4318".parse()?,
            sample_ratio: 0.1,
            incident_mode: false,
            export_timeout_seconds: 5,
            service_name: "braid".into(),
            log_format: LogFormat::Text,
        },
        profiles: vec![profile],
        profile_selection: ProfileSelection { default_pr_profile: "default".into() },
    };
    config.runtime.resolve(directory);
    config.validate()?;
    let text = toml::to_string(&config)?;
    std::fs::create_dir_all(directory)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&config_path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    println!(
        "已创建 Harness 配置：{}\n内建协作平台使用自己的配置和数据库；启动平台后迁移 Harness 数据库，再通过 braid serve 或 Web Console 启动执行。",
        config_path.display()
    );
    Ok(())
}
