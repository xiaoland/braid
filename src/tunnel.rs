use std::{path::Path, process::Stdio};

use anyhow::{Context as _, Result, bail};
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::{Child, Command},
    sync::mpsc,
    task::JoinHandle,
    time::{Duration, timeout},
};
use uuid::Uuid;

use crate::{
    config::Config,
    github::{AppWebhookConfig, GitHubClient, RepositoryName},
};

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Error)]
pub enum TunnelError {
    #[error("cannot start Wrangler Quick Tunnel: {0}")]
    Start(std::io::Error),
    #[error(
        "Wrangler Quick Tunnel did not publish and register a trycloudflare.com URL within 45 seconds"
    )]
    MissingUrl,
    #[error("cannot stop Wrangler Quick Tunnel: {0}")]
    Stop(std::io::Error),
}

pub struct QuickTunnel {
    child: Child,
    drains: Vec<JoinHandle<()>>,
    pub url: String,
}

impl QuickTunnel {
    pub async fn start(wrangler: &Path, local_url: &str) -> Result<Self, TunnelError> {
        let mut child = Command::new(wrangler)
            .args(["tunnel", "quick-start", local_url, "--log-level", "info"])
            .env("TUNNEL_TRANSPORT_PROTOCOL", "http2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(TunnelError::Start)?;
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let drains = vec![drain(stdout, sender.clone()), drain(stderr, sender)];
        let url = timeout(Duration::from_secs(45), async {
            let mut public_url = None;
            let mut registered = false;
            loop {
                tokio::select! {
                    line = receiver.recv() => {
                        let line = line?;
                        if public_url.is_none() {
                            public_url = find_tunnel_url(&line);
                        }
                        if line.contains("Registered tunnel connection") {
                            registered = true;
                        }
                        if registered && public_url.is_some() {
                            return public_url;
                        }
                    }
                    status = child.wait() => {
                        return status.ok().and_then(|status| {
                            tracing::error!(%status, "Wrangler Quick Tunnel exited during startup");
                            None
                        });
                    }
                }
            }
        })
        .await
        .ok()
        .flatten()
        .ok_or(TunnelError::MissingUrl)?;
        Ok(Self { child, drains, url })
    }

    pub fn has_exited(&mut self) -> Result<bool, TunnelError> {
        self.child.try_wait().map(|status| status.is_some()).map_err(TunnelError::Stop)
    }

    pub async fn stop(mut self) -> Result<(), TunnelError> {
        if self.child.try_wait().map_err(TunnelError::Stop)?.is_none() {
            self.child.kill().await.map_err(TunnelError::Stop)?;
        }
        for drain in self.drains.drain(..) {
            drain.abort();
        }
        Ok(())
    }
}

fn drain<R>(reader: R, sender: mpsc::UnboundedSender<String>) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            tracing::info!(target: "braid::tunnel", output = %line, "Wrangler Quick Tunnel");
            let _ = sender.send(line);
        }
    })
}

fn find_tunnel_url(line: &str) -> Option<String> {
    line.split(|character: char| {
        character.is_whitespace() || matches!(character, '|' | '"' | '\'' | '`')
    })
    .map(|value| {
        value.trim_matches(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, ':' | '/' | '.' | '-')
        })
    })
    .find(|value| value.starts_with("https://") && value.ends_with(".trycloudflare.com"))
    .map(str::to_owned)
}

pub(crate) async fn signed_public_probe(
    url: &str,
    secret: &[u8],
    repository: &str,
    repository_node_id: &str,
) -> Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({
        "zen":"Braid signed public tunnel probe",
        "repository":{"full_name":repository,"node_id":repository_node_id}
    }))?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .tls_backend_rustls()
        .build()
        .context("cannot construct public tunnel probe client")?;
    let mut last = None;
    for _ in 0..18 {
        match client
            .post(url)
            .header("X-Hub-Signature-256", &signature)
            .header("X-GitHub-Delivery", Uuid::now_v7().to_string())
            .header("X-GitHub-Event", "ping")
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => last = Some(format!("HTTP {}", response.status())),
            Err(error) => last = Some(format!("{error:?}")),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    bail!(
        "signed public tunnel probe did not converge: {}",
        last.as_deref().unwrap_or("no response")
    )
}

pub async fn start_verified_quick_tunnel(
    config: &Config,
    local_url: &str,
    secret: &[u8],
    repository: &str,
    repository_node_id: &str,
) -> Result<(QuickTunnel, String)> {
    let mut last_error = None;
    for attempt in 1..=5 {
        let started = match QuickTunnel::start(&config.tools.wrangler, local_url).await {
            Ok(started) => started,
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(attempt, error = %message, "cannot start Quick Tunnel candidate");
                last_error = Some(message);
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        // The banner appears before Cloudflare publishes the fresh hostname;
        // probing immediately primes negative DNS caches (system and
        // upstream) that outlive the probe loop. Give the record time to
        // exist before the first lookup.
        tokio::time::sleep(Duration::from_secs(20)).await;
        let public_webhook = format!("{}/webhook", started.url);
        match signed_public_probe(&public_webhook, secret, repository, repository_node_id).await {
            Ok(()) => return Ok((started, public_webhook)),
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(attempt, url = %public_webhook, error = %message, "discarding an unreachable Quick Tunnel");
                last_error = Some(message);
                if let Err(stop_error) = started.stop().await {
                    tracing::warn!(attempt, error = %stop_error, "cannot stop unreachable Quick Tunnel");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    bail!(
        "no verified Quick Tunnel became reachable after 5 candidates: {}",
        last_error.as_deref().unwrap_or("no public probe result")
    )
}

pub async fn probe_public_webhook(config: &Config, url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("public webhook URL is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(|host| !host.ends_with(".trycloudflare.com"))
        || parsed.path() != "/webhook"
    {
        bail!("public webhook URL must be an HTTPS trycloudflare.com /webhook endpoint");
    }
    let secret = config
        .webhook_secret()
        .with_context(|| "cannot load GitHub webhook secret from configured source")?;
    if secret.is_empty() {
        bail!("GitHub webhook secret must not be empty");
    }
    let repository = config.github.repository.parse::<RepositoryName>()?;
    let github = GitHubClient::connect(&config.github, &repository).await?;
    signed_public_probe(
        url,
        secret.as_bytes(),
        &config.github.repository,
        &github.identity().repository_node_id,
    )
    .await
}

pub(crate) async fn restore_webhook(github: &GitHubClient, prior: &AppWebhookConfig) -> Result<()> {
    github.update_app_webhook(&prior.url, None).await?;
    Ok(())
}
