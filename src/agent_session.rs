use std::fmt;

use tokio::sync::broadcast;

/// Snapshot returned synchronously by `status()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Idle,
    Running,
    Failed,
}

impl fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Idle => write!(f, "idle"),
            Self::Running => write!(f, "running"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

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

/// Result returned synchronously by `send_user_msg`.
#[derive(Debug, Clone)]
pub enum SendResult {
    /// A new turn was accepted; the value is the concrete provider turn id.
    Started { provider_turn_id: String },
    /// The message was accepted but does not create a new turn.
    Acknowledged,
}

/// Lifecycle events that the core needs to react to.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    TurnStarted { provider_turn_id: String },
    TurnTerminal { provider_turn_id: String, outcome: TurnOutcome },
    SessionReplaced { old_id: String, new_id: String },
    Failed { reason: String },
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
/// the event stream carries asynchronous lifecycle changes. The adapter
/// internally owns queuing, steering, and physical session replacement.
#[async_trait::async_trait]
pub trait AgentSession: Send + Sync {
    fn events(&self) -> broadcast::Receiver<SessionEvent>;

    /// Send a user message batch. The caller supplies the latest materialized
    /// context when it has one; the adapter decides whether a physical reset is
    /// required. For new turns the concrete provider turn id is returned
    /// synchronously so the caller can record it immediately.
    async fn send_user_msg(
        &self,
        msg: String,
        steering: bool,
        reset_context_to: Option<String>,
    ) -> Result<SendResult, SessionError>;
}
