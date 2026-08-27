# Agent Session Interface

## Decision

The core agent-session contract is a trait plus an event stream.

```rust
#[async_trait]
pub trait AgentSession: Send + Sync {
    fn events(&self) -> broadcast::Receiver<SessionEvent>;

    /// Dispatch a user-level message to the adapter.
    ///
    /// * `message` – rendered event references or reset context payload.
    /// * `steering` – if true, the message is an urgent steer for the currently
    ///   running turn.
    /// * `reset_context_to` – if present, the adapter must replace the
    ///   physical provider thread with a fresh session whose context is exactly
    ///   this materialized context.
    ///
    /// The call returns as soon as the adapter has accepted the message. The
    /// concrete provider turn id for a newly started turn is returned in
    /// `SendResult::started` so the caller can record it synchronously; terminal
    /// outcomes are delivered asynchronously through `events()`.
    async fn send_user_msg(
        &self,
        message: String,
        steering: bool,
        reset_context_to: Option<String>,
    ) -> Result<SendResult, SessionError>;
}

#[derive(Debug)]
pub enum SendResult {
    /// A new turn was accepted; `provider_turn_id` is the concrete provider's
    /// turn identifier.
    Started { provider_turn_id: String },
    /// The message was accepted but does not start a new turn (steer/reset).
    Acknowledged,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    TurnStarted { provider_turn_id: String },
    TurnTerminal { outcome: TurnOutcome },
    SessionReplaced { provider_session_id: String },
    Failed { error: SessionError },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    Cancelled,
    Failed,
    Interrupted,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("session is no longer connected")]
    Disconnected,
    #[error("provider rejected the message: {0}")]
    Provider(String),
    #[error("internal session error: {0}")]
    Internal(String),
}
```

## Rationale

Earlier drafts returned `Result<(), SessionError>`. That is too lossy for the
scheduler: it records `provider_turn_id` synchronously after `start_turn` and
must know the id before the event loop sees `TurnStarted`. Returning the id in
the result keeps the scheduler's synchronous book-keeping intact while still
moving all transport-level work (queueing, steering, physical session
replacement) inside the adapter.

The event stream remains the source of truth for turn terminal outcomes and for
physical-session replacement events.
