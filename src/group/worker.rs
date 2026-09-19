#![allow(clippy::large_futures)]
use std::sync::Arc;

use anyhow::{Context as _, Result};
use tokio::{
    sync::watch,
    time::{Duration, MissedTickBehavior},
};

use super::{
    SessionManager,
    provider::{materialized_profile, operational_status_unknown_profile},
};
use crate::{
    agent_session::SessionFactory,
    config::{Config, Profile},
    github::GitHubClient,
    store::{ProfileRecord, StoreActor, TurnClaim},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GroupKind {
    Issue,
    Pr,
}

impl GroupKind {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Pr => "pr",
        }
    }
}

pub(crate) struct GroupSpec {
    pub(super) kind: GroupKind,
    pub(super) profile: Profile,
    pub(super) profile_record: ProfileRecord,
}

impl GroupSpec {
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile.id
    }

    pub(crate) fn new(kind: GroupKind, config: &Config, store: &StoreActor) -> Result<Self> {
        let profile = match kind {
            GroupKind::Issue => config
                .profiles
                .iter()
                .find(|profile| profile.has_tag("issue"))
                .context("configuration has no Issue Profile")?,
            GroupKind::Pr => config.profile(&config.profile_selection.default_pr_profile)?,
        }
        .clone();
        let profile_record = materialized_profile(&profile)?;
        store.register_profile(profile_record.clone())?;
        Ok(Self { kind, profile, profile_record })
    }
}

/// Shared logical driver; adapters own physical resources and the store owns durable identities.
pub(super) struct GroupDriver<'a> {
    pub(super) store: &'a StoreActor,
    pub(super) github: &'a GitHubClient,
    pub(super) config: &'a Config,
    pub(super) spec: &'a GroupSpec,
    pub(super) sessions: SessionManager,
}

impl GroupDriver<'_> {
    async fn resume(&self) -> Result<()> {
        match self.spec.kind {
            GroupKind::Issue => self.resume_issue_provider_sessions().await,
            GroupKind::Pr => self.resume_pr_provider_sessions().await,
        }
    }

    async fn materialize_next_assignment(&self) {
        match self.spec.kind {
            GroupKind::Issue => self.materialize_next_issue_assignment().await,
            GroupKind::Pr => Box::pin(self.materialize_next_pr_assignment()).await,
        }
    }
}

pub(crate) async fn agent_group_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    config: Config,
    spec: GroupSpec,
    factory: Arc<dyn SessionFactory>,
    reports: tokio::sync::mpsc::Sender<crate::health::ProviderHealthUpdate>,
    mut shutdown: watch::Receiver<bool>,
) {
    let driver = GroupDriver {
        store: &store,
        github: &github,
        config: &config,
        spec: &spec,
        sessions: SessionManager::new(factory),
    };
    driver.drive(&reports, &mut shutdown).await;
    driver.sessions.retain(&std::collections::HashSet::new()).await;
}

impl GroupDriver<'_> {
    async fn fence_running(&self, running: &mut Option<RunningAgentTurn>) {
        if let Some(active) = running.take() {
            if let Some(reset_id) = active.reset_id {
                let _ = self.store.mark_context_reset_turn_terminal(
                    reset_id,
                    active.claim.turn_id.clone(),
                    "unknown".into(),
                );
            } else {
                let _ =
                    self.store.mark_turn_terminal(active.claim.turn_id.clone(), "unknown".into());
            }
            let _ = self.store.enqueue_operational_status(
                active.claim.turn_id,
                super::provider::operational_status_unknown_profile(&active.claim.profile_id),
            );
            self.sessions.remove(&active.claim.provider_session_id).await;
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn drive(
        &self,
        reports: &tokio::sync::mpsc::Sender<crate::health::ProviderHealthUpdate>,
        shutdown: &mut watch::Receiver<bool>,
    ) {
        let store = self.store;
        let mut running: Option<RunningAgentTurn> = None;
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut recovery = tokio::time::Instant::now();
        let mut available = false;
        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => return,
                event = async {
                    if let Some(ref mut active) = running {
                        active.events.recv().await
                    } else {
                        std::future::pending().await
                    }
                }, if running.is_some() => {
                    match event {
                        Ok(crate::agent_session::SessionEvent::TurnStarted { provider_turn_id }) => {
                            tracing::debug!(%provider_turn_id, "AgentSession turn started");
                        }
                        Ok(crate::agent_session::SessionEvent::TurnTerminal { provider_turn_id, outcome, error }) => {
                            if let Some(error) = &error {
                                tracing::warn!(%provider_turn_id, %error, "AgentSession turn terminal with error");
                            }
                            if running
                                .as_ref()
                                .is_some_and(|active| active.provider_turn_id == provider_turn_id)
                                && let Some(active) = running.take()
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
                                    if active.claim.trusted_mention && lifecycle != "unknown" {
                                        let reaction = if lifecycle == "completed" { "+1" } else { "confused" };
                                        let _ = store.enqueue_turn_reaction(active.claim.turn_id.clone(), reaction.into());
                                    }
                                }
                                if lifecycle == "unknown" {
                                    let _ = store.enqueue_operational_status(
                                        active.claim.turn_id.clone(),
                                        operational_status_unknown_profile(&active.claim.profile_id),
                                    );
                                    self.sessions.remove(&active.claim.provider_session_id).await;
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "AgentSession event consumer lagged");
                            self.fence_running(&mut running).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            self.fence_running(&mut running).await;
                        }
                    }
                }
                _ = tick.tick() => {
                    if running.as_ref().is_some_and(|active| active.events.is_closed()) {
                        self.fence_running(&mut running).await;
                    }
                    if running.is_none() && tokio::time::Instant::now() >= recovery {
                        // Resume only absent or failed handles; a sibling's recovery must
                        // not fence healthy turns or rebuild their sessions.
                        let readiness = self.sessions.check().await;
                        available = readiness.is_ok();
                        let result = match readiness {
                            Ok(()) => self.resume().await,
                            Err(error) => Err(error.into()),
                        };
                        let error = result.err().map(|error| error.to_string());
                        if let Some(error) = &error { tracing::warn!(%error, kind = self.spec.kind.as_str(), "session recovery unavailable"); }
                        if reports.send(crate::health::ProviderHealthUpdate {
                            group: self.spec.kind.as_str(), error,
                        }).await.is_err() { return; }
                        recovery = tokio::time::Instant::now() + Duration::from_secs(2);
                    }
                    if let Some(active) = &mut running {
                        self.begin_active_context_reset(active).await;
                        if active.reset_id.is_none() {
                            self.forward_urgent_steer(active).await;
                        }
                        continue;
                    }
                    if !available {
                        running = self.start_next_agent_turn().await;
                        continue;
                    }
                    let (handled_lifecycle, lifecycle_turn) = Box::pin(self.handle_next_work_item_lifecycle()).await;
                    if handled_lifecycle {
                        running = lifecycle_turn;
                        continue;
                    }
                    if Box::pin(self.materialize_next_context_reset()).await {
                        continue;
                    }
                    self.materialize_next_assignment().await;
                    running = self.start_next_agent_turn().await;
                }
            }
        }
    }
}

/// In-memory projection of the in-flight turn claim: the store is the
/// authority; this cache exists so the drive loop can attribute the terminal
/// event and fence resets without re-querying.
pub(crate) struct RunningAgentTurn {
    pub(crate) claim: TurnClaim,
    pub(crate) provider_turn_id: String,
    pub(crate) reset_id: Option<String>,
    /// The receiver that observed this turn's `TurnStarted`, created before
    /// the send and handed off with the turn, so the drive loop consumes the
    /// terminal with no subscription-timing gap.
    pub(crate) events: tokio::sync::broadcast::Receiver<crate::agent_session::SessionEvent>,
}
