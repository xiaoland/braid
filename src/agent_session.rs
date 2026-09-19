use tokio::sync::broadcast;

/// Terminal outcome of a provider turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
    Unknown,
}

impl TurnOutcome {
    pub fn lifecycle(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }
}

/// Synchronous scheduling decision returned by `send_user_msg`.
///
/// This carries no lifecycle facts: turn identity, terminal outcomes, and
/// failures are reported solely through the `SessionEvent` stream, which is
/// the single authority for everything the durable store records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResult {
    /// A new turn was accepted; observe `SessionEvent::TurnStarted` next.
    Started,
    /// The message was accepted but does not create a new turn (steering, or
    /// dropped while a turn is running).
    Acknowledged,
}

/// Lifecycle events that the core needs to react to.
///
/// Delivery guarantees, enforced by the adapter:
///
/// - Exactly one `TurnStarted` per accepted turn (the provider reports the
///   fact twice — response and notification — and the adapter deduplicates).
/// - Exactly one `TurnTerminal` per started turn, always following its
///   `TurnStarted`. Session death is no exception: the adapter synthesizes
///   `TurnTerminal { outcome: Unknown, .. }` for the in-flight turn before
///   going quiet, so a started turn is never left without a terminal.
/// - No events for messages that only return `Acknowledged`.
/// - Handle availability is observed independently through `is_unavailable`,
///   including while idle. It is latched for the lifetime of the handle;
///   restoring a durable session produces a new handle.
///
/// Delivery reliability is the consumer's side of the contract: the receiver
/// that observed `TurnStarted` (created before the send) is handed to the
/// drive loop inside `RunningAgentTurn`, so no fact can be lost to a
/// subscription-timing gap. Across restarts, resume-time fencing of orphaned
/// `starting`/`running` turns is the second authoritative path.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// A provider turn accepted and started; the single authority for the
    /// provider turn id.
    TurnStarted { provider_turn_id: String },
    /// The turn reached a terminal outcome. `error` carries the provider's
    /// reason when the outcome is `Failed` or `Unknown`.
    TurnTerminal { provider_turn_id: String, outcome: TurnOutcome, error: Option<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session failed: {0}")]
    Failed(String),
    #[error("session is unavailable")]
    Unavailable,
}

/// The core-facing Agent Session handle.
///
/// `send_user_msg` returns as soon as the adapter has accepted the message;
/// the event stream is the single authority for all lifecycle facts. The
/// adapter owns physical session mechanics (create, steer, interrupt,
/// provider notification translation). The core orchestrates context
/// replacement by starting a fresh session with materialized context through
/// `SessionManager`; there is no in-place reset message.
#[async_trait::async_trait]
pub trait AgentSession: Send + Sync {
    fn events(&self) -> broadcast::Receiver<SessionEvent>;

    /// True once this handle can no longer safely dispatch. This does not
    /// imply that the provider's persisted session has been deleted.
    fn is_unavailable(&self) -> bool;

    /// Release this handle's execution resources, best-effort interrupting
    /// its active turn. Other sessions must remain usable.
    async fn close(&self) -> Result<(), SessionError>;

    /// Send a user message batch. `steering` selects the provider steer path
    /// for a running turn; a non-steering message while a turn runs is
    /// dropped (`Acknowledged`) because the caller is expected to route it
    /// through the event queue debounce instead.
    async fn send_user_msg(&self, msg: String, steering: bool) -> Result<SendResult, SessionError>;

    /// Best-effort termination of the in-flight turn — the control-plane
    /// sibling of steering: both are immediate operations addressed to the
    /// observed active turn, but `interrupt` carries control (terminate)
    /// rather than input.
    ///
    /// Idempotent at the state-machine boundary: no in-flight turn (or a turn
    /// that already completed) is `Ok(())`. The terminal still arrives through
    /// the event stream — `Interrupted` when honored, or the natural outcome
    /// if the turn completed first — so callers never wait on a side channel.
    async fn interrupt(&self) -> Result<(), SessionError>;
}

/// An adapter-created handle and the opaque identity to bind in the durable store.
pub(crate) struct CreatedSession {
    pub(crate) id: String,
    pub(crate) session: std::sync::Arc<dyn AgentSession>,
}

/// Core-owned creation contract. Implementations own their physical topology;
/// callers supply already-selected instructions, Context and workspace.
#[async_trait::async_trait]
pub(crate) trait SessionFactory: Send + Sync {
    /// Check runtime availability even before a Work Item has a session.
    async fn check(&self) -> Result<(), SessionError>;
    async fn start(
        &self,
        profile: crate::config::Profile,
        instructions: String,
        context: String,
    ) -> Result<CreatedSession, SessionError>;
    async fn resume(
        &self,
        id: &str,
        profile: crate::config::Profile,
        instructions: String,
    ) -> Result<CreatedSession, SessionError>;
}
