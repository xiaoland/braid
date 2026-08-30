#![allow(clippy::all, clippy::pedantic)]
use std::sync::Arc;

use tokio::sync::{Mutex, broadcast};

use crate::{
    agent_session::{AgentSession, SendResult, SessionError, SessionEvent, TurnOutcome},
    config::Profile,
    provider::{AgentProvider, ProviderError, ProviderNotification},
};

/// Adapter-internal dispatch state. Not part of the core contract: the core
/// reacts to the `SessionEvent` stream, and operator-visible status is owned
/// by the durable store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStatus {
    Idle,
    Running,
    Failed,
}

/// Adapter-level wrapper that exposes the core `AgentSession` contract over the
/// lower-level `AgentProvider` primitives.
pub struct ProviderAgentSession {
    provider: Arc<dyn AgentProvider>,
    profile: Profile,
    instructions: String,
    inner: Mutex<SessionInner>,
    events: broadcast::Sender<SessionEvent>,
}

struct SessionInner {
    status: SessionStatus,
    thread_id: Option<String>,
    current_turn_id: Option<String>,
}

impl SessionInner {
    /// Record a turn start and report whether it is new. The provider reports
    /// one fact ("turn X started on this thread") through two observations —
    /// the `turn/start` response and the async notification — and the adapter
    /// must project exactly one `TurnStarted` event per turn.
    fn note_turn_started(&mut self, turn_id: &str) -> bool {
        if self.current_turn_id.as_deref() == Some(turn_id) {
            return false;
        }
        self.current_turn_id = Some(turn_id.to_owned());
        self.status = SessionStatus::Running;
        true
    }
}

impl ProviderAgentSession {
    /// Wrap the provider handle and spawn the notification listener that
    /// translates provider notifications into core `SessionEvent`s.
    fn spawn(
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        thread_id: Option<String>,
    ) -> Arc<Self> {
        let (events, _) = broadcast::channel(512);
        let session = Arc::new(Self {
            provider,
            profile,
            instructions,
            inner: Mutex::new(SessionInner {
                status: SessionStatus::Idle,
                thread_id,
                current_turn_id: None,
            }),
            events,
        });
        let listener = Arc::downgrade(&session);
        let mut notifications = session.provider.subscribe();
        tokio::spawn(async move {
            while let Ok(notification) = notifications.recv().await {
                let Some(session) = listener.upgrade() else { break };
                if session.handle_notification(notification).await {
                    break;
                }
            }
        });
        session
    }

    pub async fn start(
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        initial_context: Option<String>,
    ) -> Result<Arc<Self>, SessionError> {
        let session = Self::spawn(provider, profile, instructions, None);
        let provider_session = session
            .provider
            .start_session(&session.profile, &session.instructions)
            .await
            .map_err(map_provider_error)?;
        if let Some(context) = initial_context {
            session
                .provider
                .inject_context(&provider_session.thread_id, &context)
                .await
                .map_err(map_provider_error)?;
        }
        session.inner.lock().await.thread_id = Some(provider_session.thread_id);
        Ok(session)
    }

    pub async fn resume(
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        thread_id: &str,
    ) -> Result<Arc<Self>, SessionError> {
        let session = Self::spawn(provider, profile, instructions, Some(thread_id.to_owned()));
        session
            .provider
            .resume_session(thread_id, &session.profile, &session.instructions)
            .await
            .map_err(map_provider_error)?;
        Ok(session)
    }

    /// The current physical provider thread id, if a session exists.
    ///
    /// This is a concrete-type accessor, not part of the core `AgentSession`
    /// contract: the core persists the id in the durable store right after
    /// creation, and the store remains the authority from then on.
    pub async fn thread_id(&self) -> Option<String> {
        self.inner.lock().await.thread_id.clone()
    }

    async fn handle_notification(self: &Arc<Self>, notification: ProviderNotification) -> bool {
        let mut inner = self.inner.lock().await;
        match notification {
            ProviderNotification::TurnStarted { thread_id, turn_id } => {
                if inner.thread_id.as_ref() == Some(&thread_id) && inner.note_turn_started(&turn_id)
                {
                    let _ =
                        self.events.send(SessionEvent::TurnStarted { provider_turn_id: turn_id });
                }
                false
            }
            ProviderNotification::TurnCompleted { thread_id, turn_id, status, error } => {
                if inner.thread_id.as_ref() == Some(&thread_id) {
                    let outcome = match status.as_str() {
                        "completed" => TurnOutcome::Completed,
                        "interrupted" => TurnOutcome::Interrupted,
                        "failed" => TurnOutcome::Failed,
                        _ => TurnOutcome::Unknown,
                    };
                    inner.current_turn_id = None;
                    inner.status = SessionStatus::Idle;
                    let _ = self.events.send(SessionEvent::TurnTerminal {
                        provider_turn_id: turn_id.clone(),
                        outcome,
                    });
                    if let Some(reason) = error {
                        let _ = self.events.send(SessionEvent::Failed { reason });
                    }
                }
                false
            }
            ProviderNotification::Disconnected => {
                inner.status = SessionStatus::Failed;
                let _ = self
                    .events
                    .send(SessionEvent::Failed { reason: "provider disconnected".into() });
                true
            }
            ProviderNotification::Activity { method, thread_id, turn_id } => {
                tracing::trace!(%method, ?thread_id, ?turn_id, "provider activity");
                false
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentSession for ProviderAgentSession {
    fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    async fn send_user_msg(&self, msg: String, steering: bool) -> Result<SendResult, SessionError> {
        let mut inner = self.inner.lock().await;

        if inner.status == SessionStatus::Failed {
            return Err(SessionError::Unavailable);
        }
        if msg.is_empty() {
            return Ok(SendResult::Acknowledged);
        }

        let thread_id = inner
            .thread_id
            .clone()
            .ok_or_else(|| SessionError::Failed("no provider session".into()))?;

        if inner.status == SessionStatus::Running {
            if steering {
                let turn_id = inner
                    .current_turn_id
                    .clone()
                    .ok_or_else(|| SessionError::Failed("no active turn".into()))?;
                self.provider
                    .steer(&thread_id, &turn_id, &msg)
                    .await
                    .map_err(map_provider_error)?;
            }
            // Non-steering messages while running are dropped; the caller is
            // expected to route them through the event queue debounce so they
            // arrive when the session is idle.
            return Ok(SendResult::Acknowledged);
        }

        let turn = self
            .provider
            .start_turn(&thread_id, &self.profile, &msg)
            .await
            .map_err(map_provider_error)?;
        if inner.note_turn_started(&turn.turn_id) {
            let _ = self.events.send(SessionEvent::TurnStarted { provider_turn_id: turn.turn_id });
        }
        Ok(SendResult::Started)
    }
}

fn map_provider_error(error: ProviderError) -> SessionError {
    match error {
        ProviderError::Start(_) | ProviderError::Timeout { .. } | ProviderError::Disconnected => {
            SessionError::Unavailable
        }
        ProviderError::Protocol(message) => SessionError::Failed(message),
    }
}
