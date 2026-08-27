use std::{
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    process::Stdio,
    time::Duration,
};

use serde::Serialize;
use tokio::{process::Command, time::timeout};

use crate::{
    config::Config,
    github::{GitHubClient, RepositoryName},
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
        path_check("runtime root", config.runtime.root(), true),
        path_check(
            "database parent",
            config.runtime.database().parent().unwrap_or(Path::new("/")),
            true,
        ),
        path_check("backup directory", config.runtime.backups(), true),
        path_check("GitHub App private key", &config.github.private_key_file, false),
        secret_check("GitHub webhook secret", || config.webhook_secret()),
    ];

    let store = StoreActor::start(
        config.runtime.database().to_path_buf(),
        config.runtime.backups().to_path_buf(),
    );
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

    match config.provider_config() {
        Ok(provider_config) => {
            if let Some(codex) = provider_config.codex {
                checks.push(match inspect_codex(&codex).await {
                    Ok(identity) => match verify_identity(&identity, &codex) {
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
            }
            if let Some(pi) = provider_config.pi {
                checks.push(match command_version("Pi CLI", &pi.executable).await {
                    check if check.state == CheckState::Pass => match pi.api_key() {
                        Ok(_) => Check {
                            name: "Pi provider".into(),
                            state: CheckState::Pass,
                            detail: format!("{} (API key present)", check.detail),
                        },
                        Err(error) => Check {
                            name: "Pi provider".into(),
                            state: CheckState::Fail,
                            detail: format!("{}; {}", check.detail, error),
                        },
                    },
                    check => Check {
                        name: "Pi provider".into(),
                        state: check.state,
                        detail: check.detail,
                    },
                });
            }
        }
        Err(error) => checks.push(Check {
            name: "runtime configuration".into(),
            state: CheckState::Fail,
            detail: error.to_string(),
        }),
    }

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

async fn github_app_check(config: &Config) -> Check {
    let repository = match config.github.repository.parse::<RepositoryName>() {
        Ok(repository) => repository,
        Err(error) => {
            return Check {
                name: "GitHub App".into(),
                state: CheckState::Fail,
                detail: error.to_string(),
            };
        }
    };
    match GitHubClient::connect(&config.github, &repository).await {
        Ok(client) => {
            let identity = client.identity();
            Check {
                name: "GitHub App".into(),
                state: CheckState::Pass,
                detail: format!(
                    "{} (App {}, installation {}) can address {}",
                    identity.app_slug,
                    identity.app_id,
                    identity.installation_id,
                    identity.repository
                ),
            }
        }
        Err(error) => Check {
            name: "GitHub App".into(),
            state: if error.is_unavailable() { CheckState::Unavailable } else { CheckState::Fail },
            detail: error.to_string(),
        },
    }
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

fn secret_check<F>(name: &str, loader: F) -> Check
where
    F: FnOnce() -> Result<String, crate::config::ConfigError>,
{
    match loader() {
        Ok(_) => Check {
            name: name.into(),
            state: CheckState::Pass,
            detail: "loaded from configured source; value not inspected".into(),
        },
        Err(error) => {
            Check { name: name.into(), state: CheckState::Fail, detail: error.to_string() }
        }
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
