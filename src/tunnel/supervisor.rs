//! Own webhook connectivity independently of Agent execution and its owner lease.
use super::{QuickTunnel, restore_webhook, signed_public_probe_once, start_verified_quick_tunnel};
use crate::{
    config::Config,
    github::{AppWebhookConfig, GitHubClient},
    health::HealthSnapshot,
};
use anyhow::{Context as _, Result, bail};
use std::sync::Arc;
use tokio::{
    sync::{RwLock, watch},
    time::{Duration, Instant, MissedTickBehavior},
};

struct Supervisor {
    config: Config,
    github: Arc<GitHubClient>,
    secret: Arc<Vec<u8>>,
    health: Arc<RwLock<HealthSnapshot>>,
    prior: AppWebhookConfig,
}

pub(crate) async fn supervise(
    config: Config,
    github: Arc<GitHubClient>,
    secret: Arc<Vec<u8>>,
    health: Arc<RwLock<HealthSnapshot>>,
    initial: QuickTunnel,
    prior: AppWebhookConfig,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    Supervisor { config, github, secret, health, prior }.run(initial, shutdown).await
}

impl Supervisor {
    async fn publish(
        &self,
        state: &'static str,
        sync: &'static str,
        error: Option<String>,
        verified: Option<String>,
    ) {
        let mut health = self.health.write().await;
        health.tunnel = state;
        health.webhook_sync = sync;
        health.tunnel_error = error;
        if let Some(url) = verified {
            health.webhook_url = Some(url);
            health.tunnel_verified_at = Some(time::OffsetDateTime::now_utc().to_string());
        }
    }

    async fn sync(&self, tunnel: &QuickTunnel) -> Result<String> {
        let expected = format!("{}/webhook", tunnel.url);
        let actual = self.github.app_webhook_config().await?;
        if actual.url != expected {
            self.github
                .update_app_webhook(&expected, Some(std::str::from_utf8(&self.secret)?))
                .await?;
        }
        let confirmed = self.github.app_webhook_config().await?;
        if confirmed.url != expected {
            bail!("App webhook URL did not converge to the verified Tunnel");
        }
        Ok(expected)
    }

    async fn verify(&self, tunnel: &QuickTunnel) -> Result<bool> {
        let expected = format!("{}/webhook", tunnel.url);
        signed_public_probe_once(
            &expected,
            &self.secret,
            self.config.repository(),
            &self.github.identity().repository_node_id,
        )
        .await?;
        Ok(self.github.app_webhook_config().await?.url == expected)
    }

    #[allow(clippy::too_many_lines)]
    async fn run(self, initial: QuickTunnel, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let mut published = format!("{}/webhook", initial.url);
        let mut tunnel = Some(initial);
        let mut stop_failure = None;
        let mut synchronized = true;
        let mut failures = 0_u8;
        let mut backoff = 10_u64;
        let mut next_attempt = Instant::now();
        let mut next_probe = Instant::now() + Duration::from_secs(60);
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            tokio::select! { biased; _=shutdown.changed()=>break, _=tick.tick()=>{} }
            let failure = tunnel.as_mut().and_then(|running| {
                if running.has_exited().unwrap_or(true) {
                    Some("Tunnel process exited")
                } else {
                    running.failure()
                }
            });
            if let Some(reason) = failure {
                self.publish("unavailable", "pending", Some(reason.into()), None).await;
                if let Some(old) = tunnel.take()
                    && let Err(error) = old.stop().await
                {
                    self.publish("stop_unconfirmed", "blocked", Some(error.to_string()), None)
                        .await;
                    stop_failure = Some(anyhow::Error::from(error));
                    let _ = shutdown.changed().await;
                    break;
                }
                synchronized = false;
                next_attempt = Instant::now();
            }
            if tunnel.is_none() {
                if Instant::now() < next_attempt {
                    continue;
                }
                self.publish("verifying", "pending", None, None).await;
                let local = format!("http://{}", self.config.server.ingress);
                let started = tokio::select! {
                    biased; _=shutdown.changed()=>break,
                    result=start_verified_quick_tunnel(&self.config,&local,&self.secret,self.config.repository(),&self.github.identity().repository_node_id)=>result,
                };
                match started {
                    Ok((candidate, _public_webhook)) => {
                        tunnel = Some(candidate);
                        synchronized = false;
                    }
                    Err(error) => {
                        if matches!(
                            error.downcast_ref::<super::TunnelError>(),
                            Some(super::TunnelError::Stop(_))
                        ) {
                            self.publish(
                                "stop_unconfirmed",
                                "blocked",
                                Some(error.to_string()),
                                None,
                            )
                            .await;
                            stop_failure = Some(error);
                            let _ = shutdown.changed().await;
                            break;
                        }
                        self.publish("unavailable", "pending", Some(error.to_string()), None).await;
                        next_attempt = Instant::now() + Duration::from_secs(backoff);
                        backoff = (backoff * 2).min(120);
                        continue;
                    }
                }
            }
            let running = tunnel.as_ref().expect("candidate established above");
            if !synchronized {
                if Instant::now() < next_attempt {
                    continue;
                }
                let sync = tokio::select! {
                    biased; _=shutdown.changed()=>break,
                    result=tokio::time::timeout(Duration::from_secs(30),self.sync(running))=>result.context("App webhook synchronization timed out").and_then(|r|r),
                };
                match sync {
                    Ok(url) => {
                        published.clone_from(&url);
                        synchronized = true;
                        failures = 0;
                        backoff = 10;
                        next_probe = Instant::now() + Duration::from_secs(60);
                        self.publish("connected", "confirmed", None, Some(url)).await;
                        tracing::info!(
                            "Quick Tunnel and App webhook recovered without restarting Agent execution"
                        );
                    }
                    Err(error) => {
                        self.publish("sync_pending", "pending", Some(error.to_string()), None)
                            .await;
                        next_attempt = Instant::now() + Duration::from_secs(backoff);
                        backoff = (backoff * 2).min(120);
                    }
                }
                continue;
            }
            if Instant::now() < next_probe {
                continue;
            }
            next_probe = Instant::now() + Duration::from_secs(60);
            let verification = tokio::select! {
                biased; _=shutdown.changed()=>break,
                result=tokio::time::timeout(Duration::from_secs(20),self.verify(running))=>result.context("public webhook verification timed out").and_then(|r|r),
            };
            match verification {
                Ok(true) => {
                    failures = 0;
                    self.publish("connected", "confirmed", None, Some(published.clone())).await;
                }
                Ok(false) => {
                    synchronized = false;
                    next_attempt = Instant::now();
                    self.publish(
                        "sync_pending",
                        "mismatch",
                        Some("App webhook URL no longer names the verified Tunnel".into()),
                        None,
                    )
                    .await;
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    self.publish("degraded", "unknown", Some(error.to_string()), None).await;
                    if failures >= 3 {
                        if let Some(old) = tunnel.take()
                            && let Err(error) = old.stop().await
                        {
                            self.publish(
                                "stop_unconfirmed",
                                "blocked",
                                Some(error.to_string()),
                                None,
                            )
                            .await;
                            stop_failure = Some(anyhow::Error::from(error));
                            let _ = shutdown.changed().await;
                            break;
                        }
                        synchronized = false;
                        next_attempt = Instant::now() + Duration::from_secs(10);
                    }
                }
            }
        }
        // Do not overwrite an operator's unrelated replacement URL on shutdown.
        let owned = tunnel.as_ref().map_or(published, |t| format!("{}/webhook", t.url));
        if let Some(running) = tunnel
            && let Err(error) = running.stop().await
        {
            tracing::warn!(%error,"cannot stop Quick Tunnel during shutdown");
            stop_failure = Some(anyhow::Error::from(error));
        }
        let restoration = tokio::time::timeout(Duration::from_secs(8), async {
            if self.github.app_webhook_config().await?.url == owned {
                restore_webhook(&self.github, &self.prior).await?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .await;
        if !matches!(restoration, Ok(Ok(()))) {
            tracing::warn!("prior App webhook restoration is unconfirmed");
        }
        if let Some(error) = stop_failure {
            self.publish("stop_unconfirmed", "blocked", Some(error.to_string()), None).await;
            return Err(error.context("Quick Tunnel execution stop is unconfirmed"));
        }
        self.publish("unavailable", "stopped", None, None).await;
        Ok(())
    }
}
