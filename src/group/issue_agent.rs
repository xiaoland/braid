#![allow(clippy::large_futures)]
use std::sync::Arc;

use anyhow::Result;
use sha2::{Digest, Sha256};
use tokio::{
    sync::{RwLock, watch},
    time::{Duration, MissedTickBehavior},
};

use crate::{
    config::{Config, Profile},
    github::GitHubClient,
    group::SessionManager,
    group::dispatch::{
        forward_urgent_steer, handle_next_work_item_lifecycle, materialize_next_context_reset,
        materialize_next_issue_assignment, start_next_agent_turn,
    },
    group::provider::{
        enqueue_provider_blocked_status, issue_system_prompt, materialized_profile,
        operational_status_unknown_profile, set_provider_unavailable,
    },
    health::HealthSnapshot,
    queue::scheduler::{RunningAgentTurn, begin_active_context_reset, policy_from_config},
    store::{ProfileRecord, StoreActor},
};

pub(crate) async fn issue_agent_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    config: Config,
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
    let provider_config = match config.default_provider_config() {
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
        // The worker owns every provider connection epoch, including the
        // first: connect, rebuild the ephemeral session map from the durable
        // store (sessions bind the epoch's provider handle), resume, drive.
        let provider = loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                result = crate::provider::connect_provider(&provider_config) => {
                    match result {
                        Ok(connected) => break connected,
                        Err(error) => {
                            set_provider_unavailable(&health, &error.to_string()).await;
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                }
            }
        };
        let sessions = Arc::new(SessionManager::new());
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
                Arc::clone(&provider),
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
        health.write().await.provider = "reconnecting";
    }
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
pub(crate) async fn drive_issue_agent_connection(
    store: &StoreActor,
    github: &GitHubClient,
    config: &Config,
    provider: Arc<dyn crate::provider::AgentProvider>,
    sessions: Arc<SessionManager>,
    profile: &Profile,
    profile_record: &ProfileRecord,
    health: &RwLock<HealthSnapshot>,
    shutdown: &mut watch::Receiver<bool>,
) -> bool {
    let _ = provider;
    let mut running: Option<RunningAgentTurn> = None;
    let mut active_events: Option<
        tokio::sync::broadcast::Receiver<crate::agent_session::SessionEvent>,
    > = None;
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => return false,
            event = async {
                if let Some(ref mut rx) = active_events {
                    rx.recv().await
                } else {
                    std::future::pending().await
                }
            }, if active_events.is_some() => {
                match event {
                    Ok(crate::agent_session::SessionEvent::TurnStarted { provider_turn_id }) => {
                        tracing::debug!(%provider_turn_id, "Issue AgentSession turn started");
                    }
                    Ok(crate::agent_session::SessionEvent::TurnTerminal { provider_turn_id, outcome }) => {
                        if let Some(active) = running.take()
                            && active.provider_turn_id == provider_turn_id
                        {
                            let lifecycle = outcome.lifecycle();
                            if let Some(reset_id) = &active.reset_id {
                                let _ = store.mark_context_reset_turn_terminal(
                                    reset_id.clone(),
                                    active.claim.turn_id.clone(),
                                    lifecycle.into(),
                                );
                            } else {
                                let _ = store.mark_turn_terminal(active.claim.turn_id.clone(), lifecycle.into());
                                if active.claim.trusted_mention {
                                    let reaction = if lifecycle == "completed" { "+1" } else { "confused" };
                                    let _ = store.enqueue_turn_reaction(active.claim.turn_id.clone(), reaction.into());
                                }
                            }
                        }
                        active_events = None;
                    }
                    Ok(crate::agent_session::SessionEvent::Failed { reason }) => {
                        if let Some(active) = running.take() {
                            let _ = store.mark_turn_terminal(active.claim.turn_id.clone(), "unknown".into());
                            let _ = store.enqueue_operational_status(
                                active.claim.turn_id.clone(),
                                operational_status_unknown_profile(&active.claim.profile_id),
                            );
                            if let Some(reset_id) = active.reset_id {
                                let _ = store.fail_context_reset(reset_id, reason.clone());
                            }
                        }
                        set_provider_unavailable(health, &format!("Issue AgentSession failed: {reason}")).await;
                        return true;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "Issue AgentSession event consumer lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        if let Some(active) = running.take() {
                            let _ = store.mark_turn_terminal(active.claim.turn_id.clone(), "unknown".into());
                            let _ = store.enqueue_operational_status(
                                active.claim.turn_id,
                                operational_status_unknown_profile(&active.claim.profile_id),
                            );
                        }
                        set_provider_unavailable(health, "Issue AgentSession event stream closed").await;
                        return true;
                    }
                }
            }
            _ = tick.tick() => {
                if let Some(active) = &mut running {
                    begin_active_context_reset(store, active).await;
                    if active.reset_id.is_none() {
                        forward_urgent_steer(store, Arc::clone(&sessions), active).await;
                    }
                    continue;
                }
                let (handled_lifecycle, lifecycle_turn) = Box::pin(handle_next_work_item_lifecycle(
                    store,
                    github,
                    config,
                    Arc::clone(&provider),
                    Arc::clone(&sessions),
                    profile,
                    policy_from_config(config),
                    "issue",
                )).await;
                if handled_lifecycle {
                    running = lifecycle_turn;
                    if let Some(ref active) = running
                        && let Some(session) = sessions.get(&active.claim.provider_session_id).await
                    {
                        active_events = Some(session.events());
                    }
                    continue;
                }
                if Box::pin(materialize_next_context_reset(
                    store, github, config, Arc::clone(&provider), Arc::clone(&sessions), profile, "issue",
                ))
                .await
                {
                    continue;
                }
                materialize_next_issue_assignment(
                    store, github, config, Arc::clone(&provider), Arc::clone(&sessions), profile, profile_record,
                ).await;
                running = start_next_agent_turn(store, Arc::clone(&sessions), profile, "issue").await;
                if let Some(ref active) = running
                    && let Some(session) = sessions.get(&active.claim.provider_session_id).await
                {
                    active_events = Some(session.events());
                }
            }
        }
    }
}

pub(crate) async fn resume_issue_provider_sessions(
    store: &StoreActor,
    config: &Config,
    provider: Arc<dyn crate::provider::AgentProvider>,
    sessions: Arc<SessionManager>,
    profile: &Profile,
    profile_record: &ProfileRecord,
) -> Result<()> {
    let candidates = store.provider_resume_candidates(profile.id.clone(), "issue".into())?;
    for candidate in candidates {
        let instructions = issue_system_prompt(config, profile, candidate.number);
        let instruction_revision = hex::encode(Sha256::digest(instructions.as_bytes()));
        let compatible = candidate.repository == config.github.repository
            && candidate.profile_id == profile.id
            && candidate.profile_revision == profile_record.revision
            && candidate.instruction_revision == instruction_revision
            && profile.workspace.is_dir();
        if !compatible {
            let message = "persisted provider session is incompatible with the effective Profile";
            store.block_provider_session(candidate.provider_session_id.clone(), message.into())?;
            enqueue_provider_blocked_status(store, profile, &candidate.assignment_id)?;
            continue;
        }
        if candidate
            .active_turn_lifecycle
            .as_deref()
            .is_some_and(|lifecycle| matches!(lifecycle, "starting" | "running"))
            && let Some(turn_id) = &candidate.active_turn_id
        {
            store.mark_turn_terminal(turn_id.clone(), "unknown".into())?;
            store.enqueue_operational_status(
                turn_id.clone(),
                operational_status_unknown_profile(&profile.id),
            )?;
        }
        match sessions
            .resume(
                candidate.provider_session_id.clone(),
                Arc::clone(&provider),
                profile.clone(),
                instructions.clone(),
            )
            .await
        {
            Ok(_) => {
                store.record_provider_resume(candidate.provider_session_id.clone())?;
                tracing::info!(
                    issue = candidate.number,
                    provider_session = %candidate.provider_session_id,
                    prior_lifecycle = %candidate.session_lifecycle,
                    "resumed compatible Issue Agent provider session"
                );
            }
            Err(error @ crate::agent_session::SessionError::Unavailable) => {
                return Err(error.into());
            }
            Err(error) => {
                store.block_provider_session(
                    candidate.provider_session_id.clone(),
                    error.to_string(),
                )?;
                enqueue_provider_blocked_status(store, profile, &candidate.assignment_id)?;
                tracing::error!(
                    %error,
                    issue = candidate.number,
                    provider_session = %candidate.provider_session_id,
                    "cannot resume Issue Agent provider session"
                );
            }
        }
    }
    Ok(())
}
