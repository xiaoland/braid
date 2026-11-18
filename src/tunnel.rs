#[cfg(unix)]
mod process_group;
mod supervisor;
#[cfg(test)]
mod tests;
use std::{path::Path, process::Stdio};
pub(crate) use supervisor::supervise;

use anyhow::{Context as _, Result, bail};
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::{Child, Command},
    sync::{mpsc, watch},
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
    failure: watch::Receiver<Option<&'static str>>,
    #[cfg(unix)]
    group: process_group::OwnedGroup,
}

impl QuickTunnel {
    pub async fn start(wrangler: &Path, local_url: &str) -> Result<Self, TunnelError> {
        let mut command = Command::new(wrangler);
        command
            .args(["tunnel", "quick-start", local_url, "--log-level", "info"])
            .env("TUNNEL_TRANSPORT_PROTOCOL", "http2")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let child = command.spawn().map_err(TunnelError::Start)?;
        #[cfg(unix)]
        let group = process_group::OwnedGroup::new(&child);
        let (sender, mut receiver) = mpsc::channel(64);
        let (failure, observed_failure) = watch::channel(None);
        // Own cleanup before registration awaits: failed/cancelled construction
        // must not detach pipe drains or leave cloudflared behind its wrapper.
        let mut running = Self {
            child,
            drains: Vec::new(),
            url: String::new(),
            failure: observed_failure,
            #[cfg(unix)]
            group,
        };
        let stdout = running.child.stdout.take().expect("piped stdout");
        let stderr = running.child.stderr.take().expect("piped stderr");
        running.drains =
            vec![drain(stdout, sender.clone(), failure.clone()), drain(stderr, sender, failure)];
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
                    status = running.child.wait() => {
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
        .flatten();
        let Some(url) = url else {
            running.stop().await?;
            return Err(TunnelError::MissingUrl);
        };
        running.url = url;
        Ok(running)
    }

    pub(crate) fn failure(&self) -> Option<&'static str> {
        *self.failure.borrow()
    }

    pub fn has_exited(&mut self) -> Result<bool, TunnelError> {
        self.child.try_wait().map(|status| status.is_some()).map_err(TunnelError::Stop)
    }

    pub async fn stop(mut self) -> Result<(), TunnelError> {
        #[cfg(unix)]
        self.group.close(&mut self.child).await.map_err(TunnelError::Stop)?;
        #[cfg(not(unix))]
        if self.child.try_wait().map_err(TunnelError::Stop)?.is_none() {
            self.child.kill().await.map_err(TunnelError::Stop)?;
        }
        for drain in self.drains.drain(..) {
            drain.abort();
        }
        Ok(())
    }
}

impl Drop for QuickTunnel {
    fn drop(&mut self) {
        for drain in &self.drains {
            drain.abort();
        }
    }
}

fn drain<R>(
    reader: R,
    sender: mpsc::Sender<String>,
    failure: watch::Sender<Option<&'static str>>,
) -> JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if line.contains("Tunnel not found") {
                failure.send_replace(Some("tunnel_not_found"));
            }
            if line.contains(" ERR ")
                || line.contains(" ERROR ")
                || line.contains("Tunnel not found")
            {
                tracing::warn!(target: "braid::tunnel", output = %line, "Wrangler Quick Tunnel error");
            } else {
                tracing::info!(target: "braid::tunnel", output = %line, "Wrangler Quick Tunnel");
            }
            let _ = sender.send(line).await;
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
    let mut last = None;
    for _ in 0..18 {
        match signed_public_probe_once(url, secret, repository, repository_node_id).await {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error.to_string()),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    bail!(
        "signed public tunnel probe did not converge: {}",
        last.as_deref().unwrap_or("no response")
    )
}

async fn signed_public_probe_once(
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
    let response = client
        .post(url)
        .header("X-Hub-Signature-256", &signature)
        .header("X-GitHub-Delivery", Uuid::now_v7().to_string())
        .header("X-GitHub-Event", "ping")
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await?;
    if response.status() != reqwest::StatusCode::ACCEPTED {
        bail!("signed public ingress returned HTTP {}, expected 202", response.status());
    }
    Ok(())
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
            Err(error @ TunnelError::Stop(_)) => return Err(error.into()),
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
                // Do not create another candidate after an unconfirmed stop.
                started.stop().await?;
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
    let repository = config.repository().parse::<RepositoryName>()?;
    let github = GitHubClient::connect(config.github()?, &repository).await?;
    signed_public_probe(
        url,
        secret.as_bytes(),
        config.repository(),
        &github.identity().repository_node_id,
    )
    .await
}

pub(crate) async fn restore_webhook(github: &GitHubClient, prior: &AppWebhookConfig) -> Result<()> {
    // Shutdown must stay bounded: on an unavailable network this request would
    // otherwise hang until the OS TCP timeout and stall service stop.
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        github.update_app_webhook(&prior.url, None),
    )
    .await
    .context("restoring the prior App webhook timed out")?
    .context("cannot restore the prior App webhook")?;
    Ok(())
}
