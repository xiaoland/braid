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
    let manifest_encoded = urlencoding::encode(&manifest_json);
    let browser_url = if owner.eq_ignore_ascii_case(&gh_user) {
        format!("https://github.com/settings/apps/new?manifest={manifest_encoded}&state={state}")
    } else {
        format!(
            "https://github.com/organizations/{owner}/settings/apps/new?manifest={manifest_encoded}&state={state}"
        )
    };

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
    open_browser(&browser_url)?;
    println!("If the browser did not open, visit:\n{browser_url}\n");

    println!("Waiting for GitHub to redirect back with the manifest code...");
    let code = wait_for_code(shared).await?;
    server.abort();

    println!("Exchanging manifest code for App credentials...");
    let app: AppConversion = gh_api_json(&format!("app-manifests/{code}/conversions"), Some(""))
        .context("cannot convert GitHub App manifest")?;
    println!("Created GitHub App: {} (ID {})", app.html_url, app.id);

    let pem_path = home.join(format!("braid-of-{owner}.pem"));
    write_secret(&pem_path, app.pem.as_bytes())?;
    let secret_path = home.join(format!("braid-of-{owner}.webhook_secret"));
    write_secret(&secret_path, webhook_secret.as_bytes())?;

    let provider_block = build_provider_block(&arguments);
    let config_path = home.join("braid.toml");
    let config = build_config(&home, app.id, &arguments.repository, &pem_path, &provider_block);
    write_secret(&config_path, config.as_bytes())?;

    println!("\nWrote configuration to: {}", config_path.display());
    println!(
        "Set the webhook secret in your environment before running Braid:\n  export BRAID_WEBHOOK_SECRET=$(cat {})",
        secret_path.display()
    );
    println!("\nNext, install the App on {}:\n{}\n", arguments.repository, install_url(&app.slug));
    println!(
        "Then run:\n  braid doctor --config {}\n  braid serve --config {} --tunnel\n",
        config_path.display(),
        config_path.display()
    );

    Ok(())
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

fn build_provider_block(arguments: &SetupArguments) -> String {
    if arguments.provider == "codex" {
        return "[provider.codex]\n\
                executable = \"/Users/lanzhijiang/.braid/codex-pkg/node_modules/.bin/codex\"\n\
                home = \"/Users/lanzhijiang/.braid/provider\"\n\
                version = \"codex-cli 0.147.0-alpha.6.5\"\n\
                stable_schema_sha256 = \"7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb\"\n\
                experimental_schema_sha256 = \"a14d4878fe7b8cdd31059dbca11d7167d8cfd06effa2f7991b5364439063a5c8\"\n"
            .to_owned();
    }
    format!(
        "[provider.pi]\n\
         executable = \"{pi}\"\n\
         provider = \"deepseek\"\n\
         model = \"{model}\"\n\
         api_key_environment = \"{api_key}\"\n\
         thinking = \"high\"\n",
        pi = pi_executable_path(),
        model = arguments.model,
        api_key = arguments.api_key_environment
    )
}

fn build_config(
    home: &Path,
    app_id: u64,
    repository: &str,
    private_key_file: &Path,
    provider_block: &str,
) -> String {
    format!(
        r#"schema_version = 1

[runtime]
root = "{root}"
database = "{database}"
backups = "{backups}"
auto_migrate = false

[github]
app_id = {app_id}
repository = "{repository}"
handle = "braid"
api_version = "2022-11-28"
private_key_file = "{private_key_file}"
webhook_secret_environment = "BRAID_WEBHOOK_SECRET"
projects_v2_enabled = false

[scheduler]
quiet_seconds = 30
event_threshold = 8
reconciliation_seconds = 60

{provider_block}

[tools]
git = "{git}"
gh = "{gh}"
wrangler = "{wrangler}"

[server]
ingress = "127.0.0.1:18080"
health = "127.0.0.1:18081"

[telemetry]
endpoint = "http://127.0.0.1:4318"
sample_ratio = 0.10
incident_mode = false
export_timeout_seconds = 5
service_name = "braid"
log_format = "text"
"#,
        root = home.join("runtime").display(),
        database = home.join("runtime/state/braid.sqlite3").display(),
        backups = home.join("runtime/state/backups").display(),
        app_id = app_id,
        repository = repository,
        private_key_file = private_key_file.display(),
        provider_block = provider_block,
        git = tool_path("git", "/usr/bin/git"),
        gh = tool_path("gh", "/opt/homebrew/bin/gh"),
        wrangler = tool_path("wrangler", "wrangler"),
    )
}

fn tool_path(name: &str, fallback: &str) -> String {
    which::which(name).map_or_else(|_| fallback.to_owned(), |p| p.to_string_lossy().into_owned())
}
