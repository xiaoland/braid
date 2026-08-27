use super::*;
use crate::runtime::pr_agent::{drive_issue_agent_connection, resume_issue_provider_sessions};
use crate::runtime::provider::{materialized_profile, set_provider_unavailable};

pub(crate) async fn issue_agent_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    config: Config,
    mut provider: Arc<dyn crate::provider::AgentProvider>,
    sessions: Arc<crate::runtime::session_manager::SessionManager>,
    health: Arc<RwLock<HealthSnapshot>>,
    mut shutdown: watch::Receiver<bool>,
) {
    let Some(profile) = config.profiles.iter().find(|profile| profile.has_tag("issue")).cloned()
    else {
        set_provider_unavailable(&health, "configuration has no Issue Profile").await;
        return;
    };
    let profile_record = match materialized_profile(&profile) {
        Ok(profile) => profile,
        Err(error) => {
            set_provider_unavailable(&health, &error.to_string()).await;
            return;
        }
    };
    let provider_config = match config.provider_config() {
        Ok(config) => config,
        Err(error) => {
            set_provider_unavailable(&health, &error.to_string()).await;
            return;
        }
    };
    if let Err(error) = store.register_profile(profile_record.clone()) {
        set_provider_unavailable(&health, &error.to_string()).await;
        return;
    }

    loop {
        let convergence_failed = if let Err(error) = resume_issue_provider_sessions(
            &store,
            &config,
            Arc::clone(&provider),
            Arc::clone(&sessions),
            &profile,
            &profile_record,
        )
        .await
        {
            tracing::error!(%error, "cannot converge persisted provider sessions");
            set_provider_unavailable(&health, &error.to_string()).await;
            true
        } else {
            let mut current = health.write().await;
            current.provider = "connected";
            current.last_error = None;
            false
        };
        let disconnected = if convergence_failed {
            true
        } else {
            Box::pin(drive_issue_agent_connection(
                &store,
                &github,
                &config,
                &provider,
                Arc::clone(&sessions),
                &profile,
                &profile_record,
                &health,
                &mut shutdown,
            ))
            .await
        };
        if !disconnected || *shutdown.borrow() {
            return;
        }
        drop(provider);
        health.write().await.provider = "reconnecting";
        loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                result = crate::provider::connect_provider(&provider_config) => {
                    match result {
                        Ok(connected) => {
                            provider = connected;
                            break;
                        }
                        Err(error) => {
                            set_provider_unavailable(&health, &error.to_string()).await;
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                }
            }
        }
    }
}
