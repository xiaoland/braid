use std::{fmt, sync::Arc};

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

/// Lifecycle events that the core needs to react to.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    TurnStarted { turn_id: String },
    TurnTerminal { turn_id: String, outcome: TurnOutcome },
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
/// Callers never inspect `status()` before sending; `send_user_msg` returns
/// immediately and the adapter internally owns queuing, steering, and physical
/// session replacement. The only asynchronous signal is `events()`.
#[async_trait::async_trait]
pub trait AgentSession: Send + Sync {
    fn id(&self) -> &str;
    fn status(&self) -> SessionStatus;
    fn events(&self) -> broadcast::Receiver<SessionEvent>;

    /// Send a user message batch. The caller supplies the latest materialized
    /// Context when it has one; the adapter decides whether a physical reset is
    /// required. The returned handle is the logical session to use for the
    /// next call (usually the same `Arc`).
    async fn send_user_msg(
        &self,
        msg: String,
        steering: bool,
        reset_context_to: Option<String>,
    ) -> Result<(), SessionError>;
}
