//! Complete GitHub mention classification after durable webhook admission.
use crate::{
    github::GitHubClient,
    store::{SchedulerPolicy, StoreActor},
};
use std::sync::Arc;
use tokio::{
    sync::watch,
    time::{Duration, MissedTickBehavior},
};

pub(crate) async fn mention_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    policy: SchedulerPolicy,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Mention-authority resolution talks to GitHub; on persistent failure
    // (e.g. token expiry before the client refreshes) back off exponentially
    // instead of hammering the API every tick.
    let mut mention_failures: u32 = 0;
    let mut mention_cooldown_until = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                if tokio::time::Instant::now() >= mention_cooldown_until {
                    match store.mention_candidates(16) {
                        Ok(candidates) => {
                            let mut failed = false;
                            for candidate in candidates {
                                match github.repository_permission(&candidate.actor_login).await {
                                    Ok(role) => {
                                        let trusted = matches!(role.to_ascii_lowercase().as_str(), "maintain" | "admin");
                                        if let Err(error) = store.resolve_mention(candidate.event_id, trusted, policy) {
                                            tracing::error!(%error, "cannot resolve mention authority");
                                        }
                                    }
                                    Err(error) => {
                                        tracing::warn!(%error, actor = %candidate.actor_login, "mention authority remains unresolved");
                                        failed = true;
                                        break;
                                    }
                                }
                            }
                            if failed {
                                mention_failures = (mention_failures + 1).min(6);
                                let backoff = Duration::from_secs(2u64.pow(mention_failures).min(60));
                                mention_cooldown_until = tokio::time::Instant::now() + backoff;
                                tracing::debug!(?backoff, "mention authority resolution backing off");
                            } else {
                                mention_failures = 0;
                            }
                        }
                        Err(error) => tracing::error!(%error, "cannot load mention candidates"),
                    }
                }
            }
        }
    }
}
