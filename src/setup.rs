use std::{
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, bail};
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::get,
};
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use crate::cli::SetupArguments;

pub(crate) mod discovery;
mod logo;

use crate::config::LlmProvider;
use crate::config::{
    CONFIG_SCHEMA_VERSION, Config, GitHubConfig, LogFormat, Profile, ProfileSelection,
    RuntimeConfig, RuntimeEntry, SchedulerConfig, ServerConfig, TelemetryConfig, ToolConfig,
    load_provider_secret_file,
};
use crate::home::{InstanceEntry, UserHome, allocate_server_ports, validate_instance_key};

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct AppConversion {
    id: u64,
    slug: String,
    name: String,
    pem: String,
    webhook_secret: String,
    html_url: String,
}

#[derive(Debug, Default)]
struct CallbackState {
    code: Option<String>,
    state: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetupDraft {
    repository: String,
    instance_key: String,
    state: String,
}

fn save_setup_draft(user_home: &UserHome, draft: &SetupDraft) -> Result<PathBuf> {
    let directory = user_home.setup_drafts_dir();
    fs::create_dir_all(&directory)
        .with_context(|| format!("cannot create setup draft directory {}", directory.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&directory)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&directory, permissions)?;
    }
    let path = user_home.setup_draft_path(&draft.instance_key);
    write_secret(&path, toml::to_string(draft)?.as_bytes())?;
    Ok(path)
}

fn load_setup_draft(user_home: &UserHome, instance_key: &str) -> Result<SetupDraft> {
    let path = user_home.setup_draft_path(instance_key);
    let text = fs::read_to_string(&path)
        .with_context(|| format!("cannot read pending setup draft {}", path.display()))?;
    toml::from_str(&text)
        .with_context(|| format!("cannot parse pending setup draft {}", path.display()))
}

fn callback_code_from_url(raw_url: &str, draft: &SetupDraft) -> Result<String> {
    let url = url::Url::parse(raw_url).context("callback URL is not a valid absolute URL")?;
    let mut code = None;
    let mut state = None;
    for (name, value) in url.query_pairs() {
        match name.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            _ => {}
        }
    }
    let state = state.context("callback URL is missing state")?;
    if state != draft.state {
        bail!("callback URL state does not match the pending setup draft");
    }
    code.filter(|code| !code.is_empty()).context("callback URL is missing code")
}

#[allow(clippy::too_many_lines)]
pub async fn run(arguments: SetupArguments) -> Result<()> {
    let user_home = UserHome::resolve(Some(&arguments.user_home))?;
    user_home.ensure_dirs()?;

    let gh_user = gh_user()?;
    println!("Authenticated to GitHub as @{gh_user}");

    let (owner, repo): (&str, &str) =
        arguments.repository.split_once('/').context("repository must be OWNER/REPOSITORY")?;
    println!("Target repository: {owner}/{repo}");
    let instance_key = arguments.instance.clone().unwrap_or_else(|| owner.to_lowercase());
    validate_instance_key(&instance_key)?;
    reject_non_pi_llm_options(&arguments)?;

    let mut registry = user_home.load_registry()?;
    let instance_dir = user_home.instance_dir(&instance_key);
    if instance_dir.exists() {
        bail!(
            "instance {instance_key:?} already exists at {}; delete it to re-run setup",
            instance_dir.display()
        );
    }

    let pending_draft = if arguments.callback_url.is_some() {
        let draft = load_setup_draft(&user_home, &instance_key)?;
        if draft.repository != arguments.repository || draft.instance_key != instance_key {
            bail!("pending setup draft does not match this repository and instance");
        }
        Some(draft)
    } else {
        None
    };
    let state = pending_draft.as_ref().map_or_else(|| random_hex(16), |draft| draft.state.clone());

    let listener = if arguments.callback_url.is_none() {
        Some(tokio::net::TcpListener::bind("127.0.0.1:0").await?)
    } else {
        None
    };
    let redirect_url = listener.as_ref().map_or_else(
        || "http://127.0.0.1/callback".to_owned(),
        |listener| {
            format!(
                "http://127.0.0.1:{}/callback",
                listener.local_addr().expect("bound listener").port()
            )
        },
    );

    let manifest = json!({
        "name": format!("braid-of-{owner}"),
        "url": "https://github.com/xiaoland/braid",
        "description": format!("Braid working-memory agent for {owner}"),
        "redirect_url": redirect_url,
        "callback_urls": [],
        "public": false,
        "default_events": [
            "issues",
            "issue_comment",
            "issue_dependencies",
            "pull_request",
            "pull_request_review",
            "pull_request_review_comment",
            "pull_request_review_thread"
        ],
        "default_permissions": {
            "metadata": "read",
            "contents": "write",
            "issues": "write",
            "pull_requests": "write"
        },
        "hook_attributes": {
            "url": "https://example.com/webhook",
            "active": true
        }
    });
    let manifest_json = serde_json::to_string(&manifest)?;
    let form_action = if owner.eq_ignore_ascii_case(&gh_user) {
        format!("https://github.com/settings/apps/new?state={state}")
    } else {
        format!("https://github.com/organizations/{owner}/settings/apps/new?state={state}")
    };

    if arguments.no_browser {
        let draft = SetupDraft {
            repository: arguments.repository.clone(),
            instance_key: instance_key.clone(),
            state: state.clone(),
        };
        let draft_path = save_setup_draft(&user_home, &draft)?;
        let html_path = write_manifest_html(&form_action, &manifest_json)?;
        print_manual_guide(
            &arguments.repository,
            owner,
            &instance_key,
            user_home.root(),
            &form_action,
            html_path.as_os_str().to_str().unwrap_or(""),
            &manifest_json,
        );
        println!("Saved pending setup state to: {}", draft_path.display());
        return Ok(());
    }

    let model = arguments
        .model
        .clone()
        .context("setup requires --model when writing an instance configuration")?;
    let llm_provider = select_llm_provider(&arguments)?;
    ensure_setup_tools()?;

    let code = if let Some(callback_url) = &arguments.callback_url {
        callback_code_from_url(callback_url, pending_draft.as_ref().expect("callback draft"))?
    } else {
        let html_path = write_manifest_html(&form_action, &manifest_json)?;
        let listener = listener.expect("browser setup listener");
        let shared = Arc::new(Mutex::new(CallbackState::default()));
        let app = Router::new()
            .route("/callback", get(callback))
            .route("/", get(|| async { Html("<h1>Waiting for GitHub redirect...</h1>") }))
            .with_state(Arc::clone(&shared));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(tokio::time::sleep(Duration::from_secs(300)))
                .await;
        });

        println!("\nOpening browser to create the GitHub App...");
        open_browser(html_path.as_os_str().to_str().unwrap_or(""))?;
        println!(
            "If the browser did not open, open this file manually:\n{}\n",
            html_path.display()
        );

        println!("Waiting for GitHub to redirect back with the manifest code...");
        let code = wait_for_code(shared, &state).await?;
        server.abort();
        code
    };

    println!("Exchanging manifest code for App credentials...");
    let app: AppConversion = gh_api_json(&format!("app-manifests/{code}/conversions"), Some(""))
        .context("cannot convert GitHub App manifest")?;
    println!("Created GitHub App: {} (ID {})", app.html_url, app.id);

    let runtime = select_runtime(&arguments).await?;
    let (ingress, health) = allocate_server_ports(&registry, user_home.root())?;

    // Register the instance in memory first so a duplicate App fails before any
    // file is written; the registry is only persisted after all files exist.
    registry.insert(InstanceEntry {
        key: instance_key.clone(),
        home: instance_dir.clone(),
        github_app_id: app.id,
        repository: arguments.repository.clone(),
    })?;
    user_home.ensure_instance_dirs(&instance_key)?;
    let base_dir = instance_dir;
    let webhook_secret = random_hex(32);

    let pem_path = base_dir.join("github-app.pem");
    write_secret(&pem_path, app.pem.as_bytes())?;

    let secrets_path = base_dir.join("secrets.toml");
    let webhook_toml = format!("webhook_secret = {webhook_secret:?}\n");
    write_secret(&secrets_path, webhook_toml.as_bytes())?;

    let config_path = base_dir.join("config.toml");
    let mut config = build_config(
        &base_dir,
        app.id,
        &instance_key,
        &arguments,
        &model,
        arguments.reasoning.as_deref(),
        &pem_path,
        &secrets_path,
        runtime,
        llm_provider.clone(),
    )?;
    config.server.ingress = ingress;
    config.server.health = health;
    if let Some(runtime_entry) = config.runtimes.first_mut() {
        runtime_entry.home = Some(base_dir.join("provider").join(&runtime_entry.adapter_type));
    }
    config.validate().context("generated config failed validation")?;

    let source = base_dir.join("source");
    if let Err(error) = ensure_source_checkout(&source, &arguments.repository, &config.tools.git) {
        println!("\nWarning: {error:#}");
        println!(
            "Clone the repository manually and re-run doctor:\n  git clone https://github.com/{} {}",
            arguments.repository,
            source.display()
        );
    }
    let runtime_home = base_dir.join("provider").join(&arguments.adapter);
    bootstrap_provider_home(&runtime_home, &arguments.adapter)?;

    let config_text = toml::to_string(&config).context("cannot serialize generated config")?;
    write_secret(&config_path, config_text.as_bytes())?;

    user_home.save_registry(&registry)?;

    println!("\nWrote configuration to: {}", config_path.display());
    println!("Wrote instance secrets to: {}", secrets_path.display());
    if let Some(provider) = &llm_provider
        && let Some(api_key_file) = &provider.api_key_file
    {
        println!("Using provider secret: {}", api_key_file.display());
    }
    println!("\nNext, install the App on {}:\n{}\n", arguments.repository, install_url(&app.slug));
    println!(
        "Then run:\n  braid doctor --instance {instance_key}\n  braid serve --instance {instance_key} --tunnel\n"
    );
    println!(
        "Once serving, activate Braid on an issue with a visible @braid comment from a \
         repository admin/maintainer. (Native Issue assignment requires GitHub-side Agent \
         App provisioning and is not available to ordinary Apps.)"
    );

    let logo_path = base_dir.join(format!("braid-of-{owner}-logo.png"));
    match logo::generate(owner, &logo_path) {
        Ok(()) => {
            println!("Generated App logo: {}", logo_path.display());
            println!(
                "Upload it at https://github.com/settings/apps/{slug}/logo \
                 (or https://github.com/organizations/{owner}/settings/apps/{slug}/logo for an org).\n",
                slug = app.slug,
                owner = owner
            );
            let _ = open_browser(&format!(
                "https://github.com/settings/apps/{slug}/logo",
                slug = app.slug
            ));
        }
        Err(error) => {
            eprintln!("Could not generate App logo: {error}");
        }
    }

    if arguments.callback_url.is_some() {
        let draft_path = user_home.setup_draft_path(&instance_key);
        fs::remove_file(&draft_path).with_context(|| {
            format!("cannot remove completed setup draft {}", draft_path.display())
        })?;
    }

    Ok(())
}

fn print_manual_guide(
    repository: &str,
    owner: &str,
    instance_key: &str,
    user_home: &Path,
    form_action: &str,
    html_path: &str,
    manifest_json: &str,
) {
    let manifest_shell = manifest_json.replace('\'', "'\\''");
    let instance_dir = user_home.join("instances").join(instance_key);
    println!("\n=== Manual GitHub App creation guide ===\n");
    println!("Repository: {repository}");
    println!("App owner:  {owner}");
    println!("Instance:   {instance_key}\n");
    println!(
        "GitHub requires the manifest to be submitted as a POST request.\n\
         The easiest option is to open this generated HTML file in a browser:\n   {html_path}\n\n\
         It will auto-submit the manifest to:\n   {form_action}\n\n\
         If you cannot transfer the HTML file, you can POST manually with curl:\n\n\
            curl -X POST '{form_action}' \\\n\
              -d 'manifest={manifest_shell}'\n"
    );
    println!("Manifest JSON:\n{manifest_json}\n");
    println!(
        "Because `--no-browser` runs no local callback server, the browser redirect\n\
         to the manifest's redirect_url will fail to load. That is expected. Copy the\n\
         complete failing URL, then finish setup on this machine with the same options\n\
         and `--callback-url` instead of `--no-browser`:\n\n\
            braid setup {repository} --callback-url '<complete-redirect-url>'\n\n\
         The pending setup draft proves the state value; Braid rejects a mismatched\n\
         callback instead of converting the wrong App. Manual App credential import\n\
         remains available below for recovery:\n\n\
            gh api -X POST app-manifests/<code>/conversions\n\n\
         The JSON response contains the App's `id`, `slug`, and `pem` (private key).\n"
    );
    println!(
        "Then finish the instance manually:\n\
         - `mkdir -p {instance}/state`\n\
         - Save the `pem` value as {instance}/github-app.pem (chmod 0600).\n\
         - Generate a webhook secret (`openssl rand -hex 32`) and store it as\n\
           `webhook_secret = \"...\"` in {instance}/secrets.toml (chmod 0600).\n\
         - Copy config.example.toml to {instance}/config.toml and fill in the\n\
           absolute paths, the App `id`, and `[instance] key = \"{instance_key}\"`.\n\
         - Register the instance in {home}/registry.toml:\n\n\
           schema_version = 1\n\
           default_instance = \"{instance_key}\"\n\n\
           [[instances]]\n\
           key = \"{instance_key}\"\n\
           home = \"{instance}\"\n\
           github_app_id = <ID>\n\
           repository = \"{repository}\"\n",
        instance = instance_dir.display(),
        home = user_home.display(),
    );
    println!(
        "\nFinally, install the App on {repository}:\n\
           https://github.com/apps/braid-of-{owner}/installations/new\n\n\
         Then run `braid doctor --instance {instance_key}` and\n\
         `braid serve --instance {instance_key} --tunnel` (the tunnel step pushes\n\
         the webhook URL and secret to the App).\n"
    );
}

fn write_manifest_html(form_action: &str, manifest_json: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("braid-setup");
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("manifest-{}.html", random_hex(8)));
    let manifest_attr = manifest_json
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"><title>Braid GitHub App Manifest</title></head>
<body>
<form action="{form_action}" method="post" id="manifest-form">
  <input type="hidden" name="manifest" value="{manifest_attr}">
</form>
<script>document.getElementById('manifest-form').submit();</script>
<p>Submitting manifest to GitHub...</p>
</body>
</html>"#
    );
    fs::write(&path, html)?;
    Ok(path)
}

async fn callback(
    State(state): State<Arc<Mutex<CallbackState>>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> (StatusCode, Html<String>) {
    let mut guard = state.lock().await;
    guard.code = params.get("code").cloned();
    guard.state = params.get("state").cloned();
    (
        StatusCode::OK,
        Html(
            "<h1>Braid setup received the GitHub redirect.</h1><p>You can close this tab.</p>"
                .into(),
        ),
    )
}

async fn wait_for_code(state: Arc<Mutex<CallbackState>>, expected_state: &str) -> Result<String> {
    for _ in 0..3000 {
        {
            let guard = state.lock().await;
            if let Some(code) = guard.code.clone() {
                if guard.state.as_deref() != Some(expected_state) {
                    bail!("GitHub App manifest callback state does not match this setup");
                }
                return Ok(code);
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    bail!("timed out waiting for GitHub App manifest redirect")
}

fn gh_user() -> Result<String> {
    let output = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .context("cannot run `gh api user`; is `gh` installed and authenticated?")?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("`gh api user` failed: {err}")
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

pub(crate) fn gh_api_json<T>(endpoint: &str, payload: Option<&str>) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let mut cmd = Command::new("gh");
    cmd.args(["api", endpoint]);
    if payload.is_some() {
        cmd.args(["-X", "POST", "--input", "-"]);
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().context("cannot spawn `gh api`")?;
    if let Some(body) = payload
        && let Some(mut stdin) = child.stdin.take()
    {
        stdin.write_all(body.as_bytes())?;
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        bail!("`gh api {endpoint}` failed: {err}")
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn open_browser(url: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        let _ = Command::new("open").arg(url).spawn()?;
    } else if cfg!(target_os = "linux") {
        let _ = Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

fn install_url(slug: &str) -> String {
    format!("https://github.com/apps/{slug}/installations/new")
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

fn write_secret(path: &Path, contents: &[u8]) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("cannot write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

async fn select_runtime(arguments: &SetupArguments) -> Result<RuntimeEntry> {
    if let Some(executable) = &arguments.runtime_executable {
        return discovery::from_executable(&arguments.adapter, executable).await;
    }

    let candidates = discovery::discover(&arguments.adapter).await?;
    if candidates.is_empty() {
        eprintln!(
            "No {} runtime found on this machine.\n\n{}",
            arguments.adapter,
            discovery::install_hint(&arguments.adapter)
        );
        bail!("setup requires a {} runtime; install it and re-run", arguments.adapter);
    }

    let chosen = if candidates.len() == 1 {
        println!(
            "Found one {} runtime: {} ({})",
            arguments.adapter,
            candidates[0].executable.display(),
            candidates[0].source
        );
        candidates.into_iter().next().expect("one candidate")
    } else {
        println!("Found {} {} runtimes:", candidates.len(), arguments.adapter);
        for (index, candidate) in candidates.iter().enumerate() {
            println!(
                "  [{}] {} ({}) version {}",
                index + 1,
                candidate.executable.display(),
                candidate.source,
                candidate.version
            );
        }
        print!("Select runtime (1-{}): ", candidates.len());
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let choice: usize = input
            .trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&n| n < candidates.len())
            .ok_or_else(|| anyhow::anyhow!("invalid runtime selection"))?;
        candidates.into_iter().nth(choice).expect("valid index")
    };

    discovery::verify(chosen)
        .await
        .with_context(|| format!("cannot verify {} runtime", arguments.adapter))
}

fn reject_non_pi_llm_options(arguments: &SetupArguments) -> Result<()> {
    if arguments.adapter != "pi"
        && (arguments.llm_provider.is_some() || arguments.api_key_file.is_some())
    {
        bail!("--llm-provider and --api-key-file are only supported for the Pi adapter");
    }
    Ok(())
}

fn select_llm_provider(arguments: &SetupArguments) -> Result<Option<LlmProvider>> {
    if arguments.adapter != "pi" {
        reject_non_pi_llm_options(arguments)?;
        return Ok(None);
    }

    let id = arguments
        .llm_provider
        .clone()
        .context("Pi setup requires --llm-provider; Braid does not infer one from the adapter")?;
    let api_key_file = arguments
        .api_key_file
        .as_ref()
        .map(|path| {
            let path = path.canonicalize().context("cannot resolve Pi provider API key file")?;
            let api_key = load_provider_secret_file(&path)?;
            if api_key.trim().is_empty() {
                bail!(
                    "provider API key secret file {} must contain a non-empty provider_api_key",
                    path.display()
                );
            }
            Ok(path)
        })
        .transpose()?;

    Ok(Some(LlmProvider { id, protocol: None, api_key_file, models: vec![], allowances: vec![] }))
}

#[allow(clippy::too_many_arguments)]
fn build_config(
    base_dir: &Path,
    app_id: u64,
    instance_key: &str,
    arguments: &SetupArguments,
    model: &str,
    reasoning: Option<&str>,
    private_key_file: &Path,
    secrets_file: &Path,
    runtime: RuntimeEntry,
    llm_provider: Option<LlmProvider>,
) -> anyhow::Result<Config> {
    let runtime_version = runtime.version.clone();
    let mut config = Config {
        finite_run: None,
        schema_version: CONFIG_SCHEMA_VERSION,
        instance: crate::config::InstanceConfig {
            key: instance_key.to_owned(),
        },
        runtime: RuntimeConfig {
            root: None,
            database: None,
            backups: None,
            worktrees: None,
            auto_migrate: false,
        },
        local: None,
        builtin: None,
        loaded_from: base_dir.join("config.toml"),
        github: Some(GitHubConfig {
            app_id,
            repository: arguments.repository.clone(),
            additional_repositories: vec![],
            handle: "braid".to_owned(),
            api_version: "2022-11-28".to_owned(),
            private_key_file: private_key_file.to_path_buf(),
            webhook_secret_environment: None,
            webhook_secret_file: Some(secrets_file.to_path_buf()),
            projects_v2_enabled: false,
        }),
        scheduler: SchedulerConfig {
            quiet_seconds: 30,
            event_threshold: 8,
            reconciliation_seconds: 60,
        },
        runtimes: vec![runtime],
        llm_providers: llm_provider.clone().into_iter().collect(),
        tools: ToolConfig {
            git: required_tool_path("git")?,
            gh: required_tool_path("gh")?,
            wrangler: required_tool_path("wrangler")?,
        },
        server: ServerConfig {
            ingress: "127.0.0.1:18080".parse().expect("valid socket address"),
            health: "127.0.0.1:18081".parse().expect("valid socket address"),
        },
        telemetry: TelemetryConfig {
            endpoint: "http://127.0.0.1:4318".parse().expect("valid URL"),
            sample_ratio: 0.10,
            incident_mode: false,
            export_timeout_seconds: 5,
            service_name: "braid".to_owned(),
            log_format: LogFormat::Text,
        },
        profiles: vec![Profile {
            id: "default".to_owned(),
            display_name: "Braid Agent".to_owned(),
            tags: vec!["issue".to_owned(), "pr".to_owned()],
            adapter_type: arguments.adapter.clone(),
            // Pin the profile to the runtime that setup actually discovered:
            // config validation requires adapter_version == runtime.version,
            // so a hardcoded string breaks the moment the local runtime
            // upgrades.
            adapter_version: runtime_version,
            provider: llm_provider.map(|provider| provider.id),
            repository: None,
            model: Some(model.to_owned()),
            reasoning: reasoning.map(str::to_owned),
            user_instructions: "You are Braid, a helpful coding assistant. Work from the supplied GitHub Context, publish concise public comments, and keep descriptions and implementation state current.".to_owned(),
            skills: Vec::new(),
            native_template: None,
            workspace: None,
            github_actor_node_id: None,
            status_surfaces: vec!["issue".to_owned(), "pr".to_owned()],
            github_context_soft_ratio: 0.80,
            github_context_hard_bytes: 100_000,
        }],
        profile_selection: ProfileSelection {
            default_pr_profile: "default".to_owned(),
        },
    };
    config.runtime.resolve(base_dir);
    config.validate().context("generated config failed validation")?;
    Ok(config)
}

/// Prepare the instance-scoped provider home so the runtime can actually
/// authenticate. Codex isolates `CODEX_HOME` per instance, so its global
/// credentials do not apply: import `~/.codex/auth.json` when present,
/// otherwise print explicit login instructions. Pi uses its explicit model
/// route and, when declared, its provider API key file; it needs no home
/// bootstrap.
/// The instance source checkout: one Git clone of the configured repository,
/// shared by all Profiles as the worktree provisioning source. Clone when
/// missing or empty; never overwrite a non-Git directory.
fn ensure_source_checkout(source: &Path, repository: &str, git: &Path) -> Result<()> {
    if source.join(".git").exists() {
        println!("Source checkout already present: {}", source.display());
        return Ok(());
    }
    if source.exists() && fs::read_dir(source).ok().is_some_and(|mut d| d.next().is_some()) {
        anyhow::bail!(
            "source path {} exists but is not a Git checkout; move it aside or clone into it",
            source.display()
        );
    }
    println!("Cloning {} into {} ...", repository, source.display());
    let status = std::process::Command::new(git)
        .args(["clone", &format!("https://github.com/{repository}.git")])
        .arg(source)
        .status()
        .with_context(|| format!("cannot launch git clone for {}", source.display()))?;
    if !status.success() {
        anyhow::bail!("git clone exited with {status}");
    }
    Ok(())
}

fn bootstrap_provider_home(home: &Path, adapter_type: &str) -> Result<()> {
    fs::create_dir_all(home)
        .with_context(|| format!("cannot create provider home {}", home.display()))?;
    if adapter_type != "codex" {
        return Ok(());
    }
    let auth = home.join("auth.json");
    if auth.is_file() {
        return Ok(());
    }
    let global = dirs::home_dir().map(|dir| dir.join(".codex").join("auth.json"));
    if let Some(global) = global
        && global.is_file()
    {
        fs::copy(&global, &auth).with_context(|| {
            format!("cannot import Codex credentials from {}", global.display())
        })?;
        write_secret(&auth, &fs::read(&auth)?)?; // enforce 0600
        println!(
            "Imported Codex credentials from {} into the instance provider home.",
            global.display()
        );
    } else {
        println!(
            "Codex provider home is not authenticated yet. Before `braid serve`, run:\n  \
             CODEX_HOME={} codex login",
            home.display()
        );
    }
    Ok(())
}

fn required_tool_path(name: &str) -> Result<PathBuf> {
    which::which(name).with_context(|| {
        format!("{name} executable not found; install {name} before running braid setup")
    })
}

fn ensure_setup_tools() -> Result<()> {
    for name in ["git", "gh", "wrangler"] {
        let _ = required_tool_path(name)?;
    }
    Ok(())
}
