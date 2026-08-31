//! Scheduler: Event Queue decisions. This module never touches provider
//! sessions or connections; it owns claims, quiet-window policy, context
//! pressure policy, and store-side fencing only.
use anyhow::{Context as _, Result};

use crate::{
    config::{Config, Profile},
    context::{ContextPressure, RenderedContext},
    store::{SchedulerPolicy, StoreActor, TurnClaim},
};

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

pub(crate) fn policy_from_config(config: &Config) -> SchedulerPolicy {
    SchedulerPolicy {
        quiet_seconds: config.scheduler.quiet_seconds,
        event_threshold: config.scheduler.event_threshold,
    }
}

pub(crate) fn record_context_pressure(
    store: &StoreActor,
    assignment_id: &str,
    rendered: &RenderedContext,
    error: Option<String>,
) -> Result<()> {
    let pressure = match rendered.pressure {
        ContextPressure::Normal => "normal",
        ContextPressure::Soft => "soft",
        ContextPressure::Hard => "hard",
    };
    store.set_assignment_context_pressure(
        assignment_id.into(),
        pressure.into(),
        Some(u64::try_from(rendered.bytes).context("Context byte count exceeds u64")?),
        error,
    )?;
    Ok(())
}

pub(crate) fn enqueue_context_pressure_status(
    store: &StoreActor,
    profile: &Profile,
    assignment_id: &str,
    rendered: &RenderedContext,
) -> Result<()> {
    if profile.status_surfaces.is_empty() {
        return Ok(());
    }
    let body = match rendered.pressure {
        ContextPressure::Soft => format!(
            "> **Braid Operational Status · `{}`**\n\n\
             **GitHub Context is near the Profile limit**\n\n\
             The complete Context is {} bytes; the configured hard limit is {} bytes. Braid supplied the complete Context without truncation and allowed the Agent turn to proceed.",
            profile.id, rendered.bytes, profile.github_context_hard_bytes,
        ),
        ContextPressure::Hard => format!(
            "> **Braid Operational Status · `{}`**\n\n\
             **GitHub Context is too large**\n\n\
             The complete Context is {} bytes; the configured hard limit is {} bytes. Braid started no provider session or turn and supplied no partial, truncated, or generated summary. Reduce the GitHub Context or raise the Profile limit, then activate a new generation.",
            profile.id, rendered.bytes, profile.github_context_hard_bytes,
        ),
        ContextPressure::Normal => return Ok(()),
    };
    store.enqueue_assignment_operational_status(assignment_id.into(), body)?;
    Ok(())
}
