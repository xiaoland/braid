use std::{
    fs,
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    process::Stdio,
    time::Duration,
};

use serde::Serialize;
use time::OffsetDateTime;
use tokio::{process::Command, time::timeout};

use crate::{
    config::Config,
    protocol::{inspect_codex, verify_identity},
    store::StoreActor,
};

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckState {
    Pass,
    Fail,
    Unavailable,
}

#[derive(Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub state: CheckState,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ready: bool,
    pub checks: Vec<Check>,
}

pub async fn run(config: &Config) -> DoctorReport {
    let mut checks = vec![
        path_check("runtime root", &config.runtime.root, true),
        path_check(
            "database parent",
            config.runtime.database.parent().unwrap_or(Path::new("/")),
            true,
        ),
        path_check("backup directory", &config.runtime.backups, true),
        path_check("GitHub App private key", &config.github.private_key_file, false),
        environment_check(&config.github.webhook_secret_environment),
    ];

    let store = StoreActor::start(config.runtime.database.clone(), config.runtime.backups.clone());
    checks.push(match store.and_then(|actor| actor.status()) {
        Ok(status) => Check {
            name: "SQLite".into(),
            state: CheckState::Pass,
            detail: format!(
                "schema {}/{}, {} pending",
                status.schema_version, status.supported_schema, status.pending_migrations
            ),
        },
        Err(error) => {
            Check { name: "SQLite".into(), state: CheckState::Fail, detail: error.to_string() }
        }
    });

    checks.push(match inspect_codex(&config.provider.codex).await {
        Ok(identity) => match verify_identity(&identity, &config.provider.codex) {
            Ok(()) => Check {
                name: "Codex app-server".into(),
                state: CheckState::Pass,
                detail: identity.version,
            },
            Err(error) => Check {
                name: "Codex app-server".into(),
                state: CheckState::Fail,
                detail: format!(
                    "{error}; actual version={}, stable={}, experimental={}",
                    identity.version,
                    identity.stable_schema_sha256,
                    identity.experimental_schema_sha256
                ),
            },
        },
        Err(error) => Check {
            name: "Codex app-server".into(),
            state: CheckState::Fail,
            detail: error.to_string(),
        },
    });

    for (name, executable) in [
        ("Git", &config.tools.git),
        ("GitHub CLI", &config.tools.gh),
        ("Wrangler", &config.tools.wrangler),
    ] {
        checks.push(command_version(name, executable).await);
    }
    checks.push(github_app_check(config).await);
    checks.push(otlp_check(config));
    let ready = checks.iter().all(|check| check.state == CheckState::Pass);
    DoctorReport { ready, checks }
}

#[derive(Serialize)]
struct AppClaims {
    iat: i64,
    exp: i64,
    iss: String,
}

#[derive(serde::Deserialize)]
struct GitHubAppResponse {
    id: u64,
    slug: String,
}

#[derive(serde::Deserialize)]
struct GitHubInstallationResponse {
    id: u64,
}

async fn github_app_check(config: &Config) -> Check {
    let key = match fs::read(&config.github.private_key_file) {
        Ok(key) => key,
        Err(error) => {
            return Check {
                name: "GitHub App".into(),
                state: CheckState::Fail,
                detail: error.to_string(),
            };
        }
    };
    let encoding_key = match jsonwebtoken::EncodingKey::from_rsa_pem(&key) {
        Ok(key) => key,
        Err(error) => {
            return Check {
                name: "GitHub App".into(),
                state: CheckState::Fail,
                detail: format!("private key is not a usable RSA PEM: {error}"),
            };
        }
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let claims = AppClaims { iat: now - 60, exp: now + 540, iss: config.github.app_id.to_string() };
    let token = match jsonwebtoken::encode(
        &jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256),
        &claims,
        &encoding_key,
    ) {
        Ok(token) => token,
        Err(error) => {
            return Check {
                name: "GitHub App".into(),
                state: CheckState::Fail,
                detail: format!("cannot sign App JWT: {error}"),
            };
        }
    };
    let client = match reqwest::Client::builder()
        .user_agent(format!("braid/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return Check {
                name: "GitHub App".into(),
                state: CheckState::Fail,
                detail: error.to_string(),
            };
        }
    };
    let app = match github_get::<GitHubAppResponse>(
        &client,
        "https://api.github.com/app",
        &token,
        &config.github.api_version,
    )
    .await
    {
        Ok(app) => app,
        Err(check) => return check,
    };
    if app.id != config.github.app_id {
        return Check {
            name: "GitHub App".into(),
            state: CheckState::Fail,
            detail: format!(
                "credential resolved App {}, expected {}",
                app.id, config.github.app_id
            ),
        };
    }
    let installation_url =
        format!("https://api.github.com/repos/{}/installation", config.github.repository);
    match github_get::<GitHubInstallationResponse>(
        &client,
        &installation_url,
        &token,
        &config.github.api_version,
    )
    .await
    {
        Ok(installation) => Check {
            name: "GitHub App".into(),
            state: CheckState::Pass,
            detail: format!(
                "{} (App {}, installation {}) can address {}",
                app.slug, app.id, installation.id, config.github.repository
            ),
        },
        Err(check) => check,
    }
}

async fn github_get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    api_version: &str,
) -> Result<T, Check> {
    let response = client
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", api_version)
        .send()
        .await
        .map_err(|error| Check {
            name: "GitHub App".into(),
            state: CheckState::Unavailable,
            detail: error.to_string(),
        })?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(Check {
            name: "GitHub App".into(),
            state: CheckState::Fail,
            detail: format!("GitHub returned {status}: {}", bounded(&body, 512)),
        });
    }
    response.json().await.map_err(|error| Check {
        name: "GitHub App".into(),
        state: CheckState::Fail,
        detail: format!("GitHub response shape is unsupported: {error}"),
    })
}

fn bounded(value: &str, bytes: usize) -> &str {
    if value.len() <= bytes {
        return value;
    }
    let mut end = bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn path_check(name: &str, path: &Path, directory: bool) -> Check {
    let valid = if directory { path.is_dir() } else { path.is_file() };
    Check {
        name: name.into(),
        state: if valid { CheckState::Pass } else { CheckState::Fail },
        detail: if valid {
            path.display().to_string()
        } else {
            format!(
                "missing expected {}: {}",
                if directory { "directory" } else { "file" },
                path.display()
            )
        },
    }
}

fn environment_check(name: &str) -> Check {
    let present = std::env::var_os(name).is_some();
    Check {
        name: "webhook secret environment".into(),
        state: if present { CheckState::Pass } else { CheckState::Fail },
        detail: if present {
            format!("{name} is set; value not inspected")
        } else {
            format!("{name} is not set")
        },
    }
}

async fn command_version(name: &str, executable: &Path) -> Check {
    if !executable.is_file() {
        return Check {
            name: name.into(),
            state: CheckState::Fail,
            detail: format!("missing executable {}", executable.display()),
        };
    }
    let result = timeout(
        Duration::from_secs(10),
        Command::new(executable).arg("--version").stdin(Stdio::null()).output(),
    )
    .await;
    match result {
        Ok(Ok(output)) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            Check {
                name: name.into(),
                state: CheckState::Pass,
                detail: stdout
                    .lines()
                    .chain(stderr.lines())
                    .next()
                    .unwrap_or("version command succeeded")
                    .to_owned(),
            }
        }
        Ok(Ok(output)) => Check {
            name: name.into(),
            state: CheckState::Fail,
            detail: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        },
        Ok(Err(error)) => {
            Check { name: name.into(), state: CheckState::Fail, detail: error.to_string() }
        }
        Err(_) => Check {
            name: name.into(),
            state: CheckState::Unavailable,
            detail: "version probe timed out".into(),
        },
    }
}

fn otlp_check(config: &Config) -> Check {
    let Some(host) = config.telemetry.endpoint.host_str() else {
        return Check {
            name: "OTLP endpoint".into(),
            state: CheckState::Fail,
            detail: "endpoint has no host".into(),
        };
    };
    let port = config.telemetry.endpoint.port_or_known_default().unwrap_or(4318);
    let addresses = (host, port).to_socket_addrs();
    let result = addresses.ok().and_then(|mut addresses| {
        addresses
            .find_map(|address| TcpStream::connect_timeout(&address, Duration::from_secs(2)).ok())
    });
    Check {
        name: "OTLP endpoint".into(),
        state: if result.is_some() { CheckState::Pass } else { CheckState::Unavailable },
        detail: if result.is_some() {
            format!("{} is reachable", config.telemetry.endpoint)
        } else {
            format!("{} did not accept a TCP connection", config.telemetry.endpoint)
        },
    }
}
