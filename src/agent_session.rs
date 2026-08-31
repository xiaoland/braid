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
/// - There is no session-scoped event kind. Connection death is a
///   connection-scoped fact observed through `AgentProvider::closed()` by the
///   worker that owns the epoch, not through this per-session stream.
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

    /// Send a user message batch. `steering` selects the provider steer path
    /// for a running turn; a non-steering message while a turn runs is
    /// dropped (`Acknowledged`) because the caller is expected to route it
    /// through the event queue debounce instead.
    async fn send_user_msg(&self, msg: String, steering: bool) -> Result<SendResult, SessionError>;
}
