//! Complete GitHub mention classification after durable webhook admission.
use crate::{
    github::GitHubClient,
    store::{SchedulerPolicy, StoreActor},
};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::watch,
    time::{Duration, MissedTickBehavior},
};

pub(crate) async fn mention_worker(
    store: Arc<StoreActor>,
    github: Arc<HashMap<String, Arc<GitHubClient>>>,
    policy: SchedulerPolicy,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Mention-authority resolution talks to GitHub; on persistent failure
    // (e.g. token expiry before the client refreshes) back off exponentially
    // instead of hammering the API every tick.
    // Repository permission failures must not hold another repository's
    // trusted activations behind this repository's oldest pending mention.
    let mut cooldowns = HashMap::<String, (u32, tokio::time::Instant)>::new();
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                for (repository, client) in github.iter() {
                    let (failures, until) = cooldowns
                        .entry(repository.clone())
                        .or_insert_with(|| (0, tokio::time::Instant::now()));
                    if tokio::time::Instant::now() < *until {
                        continue;
                    }
                    match resolve_repository_mentions(&store, client, repository, policy).await {
                        Ok(()) => *failures = 0,
                        Err(error) => {
                            *failures = (*failures + 1).min(6);
                            let backoff = Duration::from_secs(2u64.pow(*failures).min(60));
                            *until = tokio::time::Instant::now() + backoff;
                            tracing::warn!(%error, %repository, ?backoff, "mention authority remains unresolved");
                        }
                    }
                }
            }
        }
    }
}

async fn resolve_repository_mentions(
    store: &StoreActor,
    github: &GitHubClient,
    repository: &str,
    policy: SchedulerPolicy,
) -> anyhow::Result<()> {
    for candidate in store.mention_candidates(repository.into(), 16)? {
        let role = github.repository_permission(&candidate.actor_login).await?;
        let trusted = matches!(role.to_ascii_lowercase().as_str(), "maintain" | "admin");
        store.resolve_mention(candidate.event_id, trusted, policy)?;
    }
    Ok(())
}
