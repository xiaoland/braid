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

use crate::config::{
    CONFIG_SCHEMA_VERSION, CodexConfig, Config, GitHubConfig, LogFormat, PiConfig, Profile,
    ProfileSelection, ProviderConfig, RuntimeConfig, SchedulerConfig, ServerConfig,
    TelemetryConfig, ToolConfig,
};

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
    let home = expand_home(&arguments.home)?;
    fs::create_dir_all(&home)?;

    let gh_user = gh_user()?;
    println!("Authenticated to GitHub as @{gh_user}");

    let (owner, repo): (&str, &str) =
        arguments.repository.split_once('/').context("repository must be OWNER/REPOSITORY")?;
    println!("Target repository: {owner}/{repo}");

    let webhook_secret = random_hex(32);
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

    let pem_path = home.join(format!("braid-of-{owner}.pem"));
    write_secret(&pem_path, app.pem.as_bytes())?;

    // Collect secrets into a single per-owner file instead of environment
    // variables, so multiple Braid instances for different owners can run on
    // the same machine without env var collisions.
    let api_key = std::env::var(&arguments.api_key_environment).with_context(|| {
        format!(
            "provider API key environment variable {} must be set during setup",
            arguments.api_key_environment
        )
    })?;
    let secrets_path = home.join(format!("braid-of-{owner}.secrets.toml"));
    let secrets_toml =
        format!("webhook_secret = {webhook_secret:?}\nprovider_api_key = {api_key:?}\n",);
    write_secret(&secrets_path, secrets_toml.as_bytes())?;

    let config_path = home.join(format!("braid-of-{owner}.toml"));
    let config = build_config(&home, app.id, &arguments, &pem_path, &secrets_path)?;
    let config_text = toml::to_string(&config).context("cannot serialize generated config")?;
    write_secret(&config_path, config_text.as_bytes())?;

    println!("\nWrote configuration to: {}", config_path.display());
    println!("Wrote secrets to: {}", secrets_path.display());
    println!("\nNext, install the App on {}:\n{}\n", arguments.repository, install_url(&app.slug));
    println!(
        "Then run:\n  braid doctor --config {}\n  braid serve --config {} --tunnel\n",
        config_path.display(),
        config_path.display()
    );

    Ok(())
}

fn print_manual_guide(
    repository: &str,
    owner: &str,
    form_action: &str,
    html_path: &str,
    manifest_json: &str,
) {
    let manifest_shell = manifest_json.replace('\'', "'\\''");
    println!("\n=== Manual GitHub App creation guide ===\n");
    println!("Repository: {repository}");
    println!("App owner:  {owner}\n");
    println!(
        "GitHub requires the manifest to be submitted as a POST request.\n\
         The easiest option is to open this generated HTML file in a browser:\n   {html_path}\n\n\
         It will auto-submit the manifest to:\n   {form_action}\n\n\
         If you cannot transfer the HTML file, you can POST manually with curl:\n\n\
            curl -X POST '{form_action}' \\\n\
              -d 'manifest={manifest_shell}'\n\n\
         Or create the App manually at https://github.com/settings/apps/new \
            (or https://github.com/organizations/{owner}/settings/apps/new for an org) \
            with the manifest JSON below.\n"
    );
    println!("Manifest JSON:\n{manifest_json}\n");
    println!(
        "After creating the App:\n\
         - Install it on {repository}: https://github.com/apps/braid-of-{owner}/installations/new\n\
         - Set the App's webhook URL to the public tunnel URL from \
           `braid serve --config ~/.braid/braid-of-{owner}.toml --tunnel` (ends in `/webhook`).\n\
         - The generated webhook secret and provider API key are stored in \
           ~/.braid/braid-of-{owner}.secrets.toml; no environment variables are required.\n\n\
         - Download the private key, save it as ~/.braid/braid-of-{owner}.pem, \
           save the secrets as ~/.braid/braid-of-{owner}.secrets.toml, \
           then run `braid setup` without `--no-browser` to capture the manifest redirect.\n"
    );
    println!(
        "If you already have the App's ID, slug, PEM, and secrets, you can \
         also write ~/.braid/braid-of-{owner}.toml manually; see docs/user-manual/setup.md.\n"
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

fn gh_api_json<T>(endpoint: &str, payload: Option<&str>) -> Result<T>
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

fn expand_home(path: &Path) -> Result<PathBuf> {
    if let Some(s) = path.to_str()
        && s.starts_with('~')
    {
        let home = dirs::home_dir().context("cannot determine home directory")?;
        Ok(home.join(&s[2..]))
    } else {
        Ok(path.to_owned())
    }
}

fn pi_executable_path() -> String {
    let candidate = "/Users/lanzhijiang/Library/pnpm/bin/pi";
    if Path::new(candidate).is_file() { candidate.to_owned() } else { "pi".to_owned() }
}

fn build_provider_config(arguments: &SetupArguments, secrets_file: &Path) -> ProviderConfig {
    if arguments.provider == "codex" {
        ProviderConfig {
            codex: Some(CodexConfig {
                executable: PathBuf::from(
                    "/Users/lanzhijiang/.braid/codex-pkg/node_modules/.bin/codex",
                ),
                home: PathBuf::from("/Users/lanzhijiang/.braid/provider"),
                version: "codex-cli 0.147.0-alpha.6.5".to_owned(),
                stable_schema_sha256:
                    "7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb".to_owned(),
                experimental_schema_sha256:
                    "a14d4878fe7b8cdd31059dbca11d7167d8cfd06effa2f7991b5364439063a5c8".to_owned(),
            }),
            pi: None,
        }
    } else {
        ProviderConfig {
            codex: None,
            pi: Some(PiConfig {
                executable: PathBuf::from(pi_executable_path()),
                provider: Some("deepseek".to_owned()),
                model: Some(arguments.model.clone()),
                api_key_environment: Some(arguments.api_key_environment.clone()),
                api_key_file: Some(secrets_file.to_path_buf()),
                thinking: Some("high".to_owned()),
                home: Some(PathBuf::from("/Users/lanzhijiang/.braid/pi")),
            }),
        }
    }
}

fn build_config(
    home: &Path,
    app_id: u64,
    arguments: &SetupArguments,
    private_key_file: &Path,
    secrets_file: &Path,
) -> anyhow::Result<Config> {
    let runtime_root = home.join("runtime");
    let config = Config {
        schema_version: CONFIG_SCHEMA_VERSION,
        runtime: RuntimeConfig {
            root: runtime_root.clone(),
            database: runtime_root.join("state/braid.sqlite3"),
            backups: runtime_root.join("state/backups"),
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
        provider: build_provider_config(arguments, secrets_file),
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
            provider: arguments.provider.clone(),
            model: Some(arguments.model.clone()),
            reasoning: Some("high".to_owned()),
            user_instructions: "You are Braid, a helpful coding assistant. Work from the supplied GitHub Context, publish concise public comments, and keep descriptions and implementation state current.".to_owned(),
            workspace: home.join("profiles/default"),
            github_actor_node_id: None,
            status_surfaces: vec!["issue".to_owned(), "pr".to_owned()],
            github_context_soft_ratio: 0.80,
            github_context_hard_bytes: 100_000,
        }],
        profile_selection: ProfileSelection {
            default_pr_profile: "default".to_owned(),
        },
    };
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
            home: PathBuf::from("/tmp/braid-setup-test"),
            no_browser: false,
        }
    }

    #[test]
    fn generated_config_loads_for_pi() {
        let home = PathBuf::from("/tmp/braid-setup-test");
        fs::create_dir_all(&home).unwrap();
        let secrets_path = home.join("braid-of-xiaoland.secrets.toml");
        std::fs::write(&secrets_path, "webhook_secret = \"test\"\nprovider_api_key = \"key\"\n")
            .unwrap();
        let config = build_config(
            &home,
            123_456,
            &args("xiaoland/braid", "pi", "deepseek-chat"),
            &home.join("key.pem"),
            &secrets_path,
        )
        .expect("config should build");
        let text = toml::to_string(&config).expect("config should serialize");
        let parsed: Config = toml::from_str(&text).expect("serialized config should parse");
        assert_eq!(parsed.schema_version, 1);
        assert!(!parsed.profiles.is_empty());
        assert_eq!(parsed.profile_selection.default_pr_profile, "default");
    }

    #[test]
    fn generated_config_loads_for_codex() {
        let home = PathBuf::from("/tmp/braid-setup-test");
        fs::create_dir_all(&home).unwrap();
        let secrets_path = home.join("braid-of-xiaoland.secrets.toml");
        std::fs::write(&secrets_path, "webhook_secret = \"test\"\nprovider_api_key = \"key\"\n")
            .unwrap();
        let config = build_config(
            &home,
            123_456,
            &args("xiaoland/braid", "codex", "gpt-4o"),
            &home.join("key.pem"),
            &secrets_path,
        )
        .expect("codex config should build");
        let text = toml::to_string(&config).expect("config should serialize");
        let parsed: Config = toml::from_str(&text).expect("serialized codex config should parse");
        assert_eq!(parsed.schema_version, 1);
        assert!(!parsed.profiles.is_empty());
    }
}
