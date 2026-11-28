//! Own webhook connectivity independently of Agent execution and its owner lease.
use super::{QuickTunnel, signed_public_probe_once, start_verified_quick_tunnel};
use crate::{config::Config, github::GitHubClient, health::HealthSnapshot};
use anyhow::{Context as _, Result, bail};
use std::{collections::HashSet, sync::Arc};
use tokio::{
    sync::{RwLock, watch},
    time::{Duration, Instant, MissedTickBehavior},
};

struct Supervisor {
    config: Config,
    github: Arc<GitHubClient>,
    secret: Arc<Vec<u8>>,
    health: Arc<RwLock<HealthSnapshot>>,
}

enum CheckFailure {
    PublicProbe(anyhow::Error),
    AppWebhook(anyhow::Error),
}

/// Keep every candidate this runtime may have installed. A PATCH can take
/// effect even if cancellation or a network error prevents its confirmation.
#[derive(Default)]
struct WebhookHandoff {
    prior_url: Option<String>,
    attempted_urls: HashSet<String>,
}

impl WebhookHandoff {
    fn record_patch(&mut self, actual: &str, expected: &str) {
        self.prior_url.get_or_insert_with(|| actual.to_owned());
        self.attempted_urls.insert(expected.to_owned());
    }

    fn restoration_url(&self, actual: &str) -> Option<&str> {
        self.prior_url
            .as_deref()
            .filter(|prior| *prior != actual && self.attempted_urls.contains(actual))
    }

    async fn restore(&self, github: &GitHubClient) -> Result<()> {
        if self.prior_url.is_none() {
            return Ok(());
        }
        let actual = github.app_webhook_config().await?;
        if let Some(prior_url) = self.restoration_url(&actual.url) {
            github.update_app_webhook(prior_url, None).await?;
            if github.app_webhook_config().await?.url != prior_url {
                bail!("App webhook URL did not converge to its prior URL");
            }
        }
        Ok(())
    }
}

pub(crate) async fn supervise(
    config: Config,
    github: Arc<GitHubClient>,
    secret: Arc<Vec<u8>>,
    health: Arc<RwLock<HealthSnapshot>>,
    shutdown: watch::Receiver<bool>,
) -> Result<()> {
    Supervisor { config, github, secret, health }.run(shutdown).await
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

    async fn sync(&self, expected: &str, handoff: &mut WebhookHandoff) -> Result<()> {
        let actual = self.github.app_webhook_config().await?;
        if actual.url != expected || !handoff.attempted_urls.contains(expected) {
            let secret = std::str::from_utf8(&self.secret).context("webhook secret is UTF-8")?;
            handoff.record_patch(&actual.url, expected);
            self.github.update_app_webhook(expected, Some(secret)).await?;
            if self.github.app_webhook_config().await?.url != expected {
                bail!("App webhook URL did not converge to the verified Tunnel");
            }
        }
        Ok(())
    }

    async fn probe(&self, expected: &str) -> Result<()> {
        signed_public_probe_once(
            expected,
            &self.secret,
            self.config.repository(),
            &self.github.identity().repository_node_id,
        )
        .await
    }

    #[allow(clippy::too_many_lines)]
    async fn run(self, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let mut handoff = WebhookHandoff::default();
        let mut tunnel: Option<QuickTunnel> = None;
        let mut synchronized = false;
        let mut failures = 0_u8;
        let mut backoff = 10_u64;
        let mut next_attempt = Instant::now();
        let mut next_probe = Instant::now() + Duration::from_secs(60);
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut stop_failure = loop {
            tokio::select! { biased; _=shutdown.changed()=>break None, _=tick.tick()=>{} }
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
                    break Some(error.into());
                }
                synchronized = false;
                next_attempt = Instant::now();
            }
            let candidate_started = if tunnel.is_none() {
                if Instant::now() < next_attempt {
                    continue;
                }
                let started = start_verified_quick_tunnel(
                    &self.config,
                    &self.secret,
                    &self.github.identity().repository_node_id,
                    &mut shutdown,
                )
                .await;
                match started {
                    Ok(Some(candidate)) => {
                        tunnel = Some(candidate);
                        synchronized = false;
                        true
                    }
                    Ok(None) => break None,
                    Err(error) => {
                        if matches!(
                            error.downcast_ref::<super::TunnelError>(),
                            Some(super::TunnelError::Stop(_))
                        ) {
                            break Some(error);
                        }
                        tracing::warn!(%error, "Quick Tunnel candidate failed, retrying");
                        self.publish("degraded", "pending", Some(error.to_string()), None).await;
                        next_attempt = Instant::now() + Duration::from_secs(backoff);
                        backoff = (backoff * 2).min(120);
                        continue;
                    }
                }
            } else {
                false
            };
            let check_at = if synchronized { next_probe } else { next_attempt };
            if Instant::now() < check_at {
                continue;
            }
            let expected = tunnel.as_ref().expect("candidate established above").webhook_url();
            let check = async {
                // A newly established candidate already passed the signed probe.
                if !candidate_started {
                    self.probe(&expected).await.map_err(CheckFailure::PublicProbe)?;
                }
                if synchronized {
                    let actual = self
                        .github
                        .app_webhook_config()
                        .await
                        .map_err(|error| CheckFailure::AppWebhook(error.into()))?;
                    Ok(actual.url == expected)
                } else {
                    self.sync(&expected, &mut handoff).await.map_err(CheckFailure::AppWebhook)?;
                    Ok(true)
                }
            };
            let check_budget = Duration::from_secs(if synchronized { 20 } else { 30 });
            let verification = tokio::select! {
                biased; _=shutdown.changed()=>break None,
                result=tokio::time::timeout(check_budget,check)=>result
                    .context("checking the App webhook timed out")
                    .map_err(CheckFailure::AppWebhook)
                    .and_then(|result| result),
            };
            match verification {
                Ok(true) => {
                    if !synchronized {
                        tracing::info!("Quick Tunnel and App webhook connected");
                    }
                    synchronized = true;
                    failures = 0;
                    backoff = 10;
                    next_probe = Instant::now() + Duration::from_secs(60);
                    self.publish("connected", "confirmed", None, Some(expected)).await;
                }
                Ok(false) => {
                    synchronized = false;
                    failures = 0;
                    next_attempt = Instant::now();
                    self.publish(
                        "sync_pending",
                        "mismatch",
                        Some("App webhook URL no longer names the verified Tunnel".into()),
                        None,
                    )
                    .await;
                }
                Err(CheckFailure::AppWebhook(error)) => {
                    failures = 0;
                    if synchronized {
                        self.publish("degraded", "unknown", Some(error.to_string()), None).await;
                        next_probe = Instant::now() + Duration::from_secs(60);
                    } else {
                        tracing::warn!(%error, "Quick Tunnel webhook synchronization pending");
                        self.publish("sync_pending", "pending", Some(error.to_string()), None)
                            .await;
                        next_attempt = Instant::now() + Duration::from_secs(backoff);
                        backoff = (backoff * 2).min(120);
                    }
                }
                Err(CheckFailure::PublicProbe(error)) => {
                    failures = failures.saturating_add(1);
                    self.publish("degraded", "unknown", Some(error.to_string()), None).await;
                    if failures >= 3 {
                        if let Some(old) = tunnel.take()
                            && let Err(error) = old.stop().await
                        {
                            break Some(error.into());
                        }
                        synchronized = false;
                        next_attempt = Instant::now() + Duration::from_secs(10);
                    } else if synchronized {
                        next_probe = Instant::now() + Duration::from_secs(60);
                    } else {
                        next_attempt = Instant::now() + Duration::from_secs(backoff);
                        backoff = (backoff * 2).min(120);
                    }
                }
            }
        };
        if let Some(error) = &stop_failure {
            self.publish("stop_unconfirmed", "blocked", Some(error.to_string()), None).await;
            if !*shutdown.borrow() {
                let _ = shutdown.changed().await;
            }
        }
        if let Some(running) = tunnel
            && let Err(error) = running.stop().await
        {
            tracing::warn!(%error,"cannot stop Quick Tunnel during shutdown");
            stop_failure = Some(anyhow::Error::from(error));
        }
        let restoration =
            tokio::time::timeout(Duration::from_secs(8), handoff.restore(&self.github))
                .await
                .context("restoring the prior App webhook timed out")
                .and_then(|result| result);
        if let Err(error) = restoration {
            tracing::warn!(%error, "prior App webhook restoration is unconfirmed");
        }
        if let Some(error) = stop_failure {
            self.publish("stop_unconfirmed", "blocked", Some(error.to_string()), None).await;
            return Err(error.context("Quick Tunnel execution stop is unconfirmed"));
        }
        self.publish("unavailable", "stopped", None, None).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::WebhookHandoff;

    #[test]
    fn shutdown_restores_the_original_url_after_an_unconfirmed_patch() {
        let mut handoff = WebhookHandoff::default();
        assert_eq!(handoff.restoration_url("candidate-1"), None);

        handoff.record_patch("original", "candidate-1");
        handoff.record_patch("candidate-1", "candidate-2");

        assert_eq!(handoff.restoration_url("candidate-1"), Some("original"));
        assert_eq!(handoff.restoration_url("candidate-2"), Some("original"));
        assert_eq!(handoff.restoration_url("original"), None);
        assert_eq!(handoff.restoration_url("operator-update"), None);
    }
}
