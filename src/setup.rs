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
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::cli::SetupArguments;

mod discovery;
mod logo;

use crate::config::{
    CONFIG_SCHEMA_VERSION, Config, GitHubConfig, LogFormat, Profile, ProfileSelection,
    ProviderSecretFile, RuntimeConfig, RuntimeEntry, SchedulerConfig, ServerConfig,
    TelemetryConfig, ToolConfig,
};
use crate::config::{LlmAllowance, LlmModel, LlmProvider};
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

    let mut registry = user_home.load_registry()?;
    let instance_dir = user_home.instance_dir(&instance_key);
    if instance_dir.exists() {
        bail!(
            "instance {instance_key:?} already exists at {}; delete it to re-run setup",
            instance_dir.display()
        );
    }

    let state = random_hex(16);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let redirect_url = format!("http://127.0.0.1:{port}/callback");

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
            "pull_request",
            "pull_request_review",
            "pull_request_review_comment"
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
        return Ok(());
    }

    let html_path = write_manifest_html(&form_action, &manifest_json)?;

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
    println!("If the browser did not open, open this file manually:\n{}\n", html_path.display());

    println!("Waiting for GitHub to redirect back with the manifest code...");
    let code = wait_for_code(shared).await?;
    server.abort();

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

    let provider_secret_path =
        write_provider_secret(&user_home, &runtime.adapter_type, &arguments.api_key_environment)?;

    let llm_provider = build_llm_provider(
        &runtime.adapter_type,
        &arguments.model,
        &arguments.api_key_environment,
        &provider_secret_path,
    );

    let config_path = base_dir.join("config.toml");
    let mut config = build_config(
        &base_dir,
        app.id,
        &instance_key,
        &arguments,
        &pem_path,
        &secrets_path,
        runtime,
        llm_provider,
    )?;
    config.server.ingress = ingress;
    config.server.health = health;
    if let Some(runtime_entry) = config.runtimes.first_mut() {
        runtime_entry.home = Some(base_dir.join("provider").join(&runtime_entry.adapter_type));
    }
    config.validate().context("generated config failed validation")?;

    let config_text = toml::to_string(&config).context("cannot serialize generated config")?;
    write_secret(&config_path, config_text.as_bytes())?;

    user_home.save_registry(&registry)?;

    println!("\nWrote configuration to: {}", config_path.display());
    println!("Wrote instance secrets to: {}", secrets_path.display());
    println!("Wrote provider secret to: {}", provider_secret_path.display());
    println!("\nNext, install the App on {}:\n{}\n", arguments.repository, install_url(&app.slug));
    println!(
        "Then run:\n  braid doctor --instance {instance_key}\n  braid serve --instance {instance_key} --tunnel\n"
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
         to the manifest's redirect_url will fail to load. That is expected: copy the\n\
         `code` query parameter from the failing address bar, then exchange it:\n\n\
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

async fn wait_for_code(state: Arc<Mutex<CallbackState>>) -> Result<String> {
    for _ in 0..3000 {
        {
            let guard = state.lock().await;
            if let Some(code) = guard.code.clone() {
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
    if let Some(api_url) = &arguments.runtime_api_url {
        return discovery::from_api_url(&arguments.provider, api_url.clone());
    }
    if let Some(executable) = &arguments.runtime_executable {
        return discovery::from_executable(&arguments.provider, executable).await;
    }

    let candidates = discovery::discover(&arguments.provider).await?;
    if candidates.is_empty() {
        eprintln!(
            "No {} runtime found on this machine.\n\n{}",
            arguments.provider,
            discovery::install_hint(&arguments.provider)
        );
        bail!("setup requires a {} runtime; install it and re-run", arguments.provider);
    }

    let chosen = if candidates.len() == 1 {
        println!(
            "Found one {} runtime: {} ({})",
            arguments.provider,
            candidates[0].executable.display(),
            candidates[0].source
        );
        candidates.into_iter().next().expect("one candidate")
    } else {
        println!("Found {} {} runtimes:", candidates.len(), arguments.provider);
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
        .with_context(|| format!("cannot verify {} runtime", arguments.provider))
}

fn provider_secret_path(user_home: &UserHome, adapter_type: &str) -> PathBuf {
    let provider_id = if adapter_type == "codex" { "openai" } else { "deepseek" };
    user_home.secrets_dir().join(format!("{provider_id}.toml"))
}

#[allow(clippy::collapsible_if)]
fn write_provider_secret(
    user_home: &UserHome,
    adapter_type: &str,
    api_key_environment: &str,
) -> Result<PathBuf> {
    let path = provider_secret_path(user_home, adapter_type);
    if let Ok(text) = fs::read_to_string(&path) {
        if let Ok(file) = toml::from_str::<ProviderSecretFile>(&text) {
            if !file.provider_api_key.is_empty() {
                return Ok(path);
            }
        }
    }

    let api_key = if adapter_type == "pi" {
        std::env::var(api_key_environment).with_context(|| {
            format!(
                "provider API key environment variable {api_key_environment} must be set during setup, \
                 or the user secret file {} must already contain provider_api_key",
                path.display()
            )
        })?
    } else {
        std::env::var(api_key_environment).unwrap_or_default()
    };
    let toml = format!("provider_api_key = {api_key:?}\n");
    write_secret(&path, toml.as_bytes())?;
    Ok(path)
}

fn build_llm_provider(
    adapter_type: &str,
    model: &str,
    api_key_environment: &str,
    provider_secret_file: &Path,
) -> LlmProvider {
    let (id, protocol) = if adapter_type == "codex" {
        ("openai".to_owned(), "openai".to_owned())
    } else {
        ("deepseek".to_owned(), "openai-compatible".to_owned())
    };
    LlmProvider {
        id,
        protocol,
        api_key_environment: Some(api_key_environment.to_owned()),
        api_key_file: Some(provider_secret_file.to_path_buf()),
        models: vec![LlmModel {
            model_id: model.to_owned(),
            input_cost: 0.0,
            output_cost: 0.0,
            cache_input_cost: 0.0,
        }],
        allowances: vec![LlmAllowance {
            since: "2026-01-01".to_owned(),
            until: "2027-01-01".to_owned(),
            amount: 0.0,
        }],
    }
}

#[allow(clippy::too_many_arguments)]
fn build_config(
    base_dir: &Path,
    app_id: u64,
    instance_key: &str,
    arguments: &SetupArguments,
    private_key_file: &Path,
    secrets_file: &Path,
    runtime: RuntimeEntry,
    llm_provider: LlmProvider,
) -> anyhow::Result<Config> {
    let runtime_version = runtime.version.clone();
    let mut config = Config {
        schema_version: CONFIG_SCHEMA_VERSION,
        instance: crate::config::InstanceConfig {
            key: instance_key.to_owned(),
        },
        runtime: RuntimeConfig {
            root: None,
            database: None,
            backups: None,
            auto_migrate: false,
        },
        github: GitHubConfig {
            app_id,
            repository: arguments.repository.clone(),
            handle: "braid".to_owned(),
            api_version: "2022-11-28".to_owned(),
            private_key_file: private_key_file.to_path_buf(),
            webhook_secret_environment: None,
            webhook_secret_file: Some(secrets_file.to_path_buf()),
            projects_v2_enabled: false,
        },
        scheduler: SchedulerConfig {
            quiet_seconds: 30,
            event_threshold: 8,
            reconciliation_seconds: 60,
        },
        runtimes: vec![runtime],
        llm_providers: vec![llm_provider],
        tools: ToolConfig {
            git: PathBuf::from(tool_path("git", "/usr/bin/git")),
            gh: PathBuf::from(tool_path("gh", "/opt/homebrew/bin/gh")),
            wrangler: PathBuf::from(tool_path("wrangler", "wrangler")),
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
            adapter_type: arguments.provider.clone(),
            // Pin the profile to the runtime that setup actually discovered:
            // config validation requires adapter_version == runtime.version,
            // so a hardcoded string breaks the moment the local runtime
            // upgrades.
            adapter_version: runtime_version,
            provider: if arguments.provider == "codex" {
                "openai".to_owned()
            } else {
                "deepseek".to_owned()
            },
            model: Some(arguments.model.clone()),
            reasoning: Some("high".to_owned()),
            user_instructions: "You are Braid, a helpful coding assistant. Work from the supplied GitHub Context, publish concise public comments, and keep descriptions and implementation state current.".to_owned(),
            workspace: base_dir.join("workspace/default"),
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

fn tool_path(name: &str, fallback: &str) -> String {
    which::which(name).map_or_else(|_| fallback.to_owned(), |p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn args(repo: &str, provider: &str, model: &str) -> SetupArguments {
        SetupArguments {
            repository: repo.to_owned(),
            provider: provider.to_owned(),
            model: model.to_owned(),
            api_key_environment: "DEEPSEEK_API_KEY".to_owned(),
            instance: None,
            user_home: PathBuf::from("/tmp/braid-setup-test"),
            runtime_executable: None,
            runtime_api_url: None,
            no_browser: false,
        }
    }

    #[test]
    fn generated_config_loads_for_pi() {
        let home = PathBuf::from("/tmp/braid-setup-test");
        fs::create_dir_all(&home).unwrap();
        let webhook_secret_path = home.join("secrets.toml");
        std::fs::write(&webhook_secret_path, "webhook_secret = \"test\"\n").unwrap();
        let provider_secret_path = home.join("secrets/deepseek.toml");
        std::fs::create_dir_all(provider_secret_path.parent().unwrap()).unwrap();
        std::fs::write(&provider_secret_path, "provider_api_key = \"key\"\n").unwrap();
        let runtime = RuntimeEntry {
            adapter_type: "pi".to_owned(),
            version: "0.84.3".to_owned(),
            executable: PathBuf::from("/tmp/pi"),
            api_url: None,
            home: None,
            stable_schema_sha256: None,
            experimental_schema_sha256: None,
        };
        let llm_provider =
            build_llm_provider("pi", "deepseek-chat", "DEEPSEEK_API_KEY", &provider_secret_path);
        let config = build_config(
            &home,
            123_456,
            "xiaoland",
            &args("xiaoland/braid", "pi", "deepseek-chat"),
            &home.join("key.pem"),
            &webhook_secret_path,
            runtime,
            llm_provider,
        )
        .expect("config should build");
        let text = toml::to_string(&config).expect("config should serialize");
        let parsed: Config = toml::from_str(&text).expect("serialized config should parse");
        assert_eq!(parsed.schema_version, 2);
        assert!(!parsed.runtimes.is_empty());
        assert!(!parsed.llm_providers.is_empty());
        assert!(!parsed.profiles.is_empty());
        assert_eq!(parsed.profile_selection.default_pr_profile, "default");
    }

    #[test]
    fn generated_config_loads_for_codex() {
        let home = PathBuf::from("/tmp/braid-setup-test");
        fs::create_dir_all(&home).unwrap();
        let webhook_secret_path = home.join("secrets.toml");
        std::fs::write(&webhook_secret_path, "webhook_secret = \"test\"\n").unwrap();
        let provider_secret_path = home.join("secrets/openai.toml");
        std::fs::create_dir_all(provider_secret_path.parent().unwrap()).unwrap();
        std::fs::write(&provider_secret_path, "provider_api_key = \"key\"\n").unwrap();
        let runtime = RuntimeEntry {
            adapter_type: "codex".to_owned(),
            version: "codex-cli 0.147.0-alpha.6.5".to_owned(),
            executable: PathBuf::from("/tmp/codex"),
            api_url: None,
            home: Some(home.join("provider")),
            stable_schema_sha256: Some(
                "7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb".to_owned(),
            ),
            experimental_schema_sha256: Some(
                "a14d4878fe7b8cdd31059dbca11d7167d8cfd06effa2f7991b5364439063a5c8".to_owned(),
            ),
        };
        let llm_provider =
            build_llm_provider("codex", "gpt-4o", "OPENAI_API_KEY", &provider_secret_path);
        let config = build_config(
            &home,
            123_456,
            "xiaoland",
            &args("xiaoland/braid", "codex", "gpt-4o"),
            &home.join("key.pem"),
            &webhook_secret_path,
            runtime,
            llm_provider,
        )
        .expect("codex config should build");
        let text = toml::to_string(&config).expect("config should serialize");
        let parsed: Config = toml::from_str(&text).expect("serialized codex config should parse");
        assert_eq!(parsed.schema_version, 2);
        assert!(!parsed.runtimes.is_empty());
        assert!(!parsed.llm_providers.is_empty());
        assert!(!parsed.profiles.is_empty());
    }
}
