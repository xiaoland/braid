#![allow(clippy::all, clippy::pedantic)]
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
};

use tokio::sync::{Mutex, broadcast};

use crate::{
    agent_session::{AgentSession, SessionError, SessionEvent, SessionStatus, TurnOutcome},
    config::Profile,
    provider::{AgentProvider, ProviderError, ProviderNotification},
};

/// Adapter-level wrapper that exposes the core `AgentSession` contract over the
/// lower-level `AgentProvider` primitives.
pub struct ProviderAgentSession {
    id: String,
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
    pending_reset: Option<PendingReset>,
    last_context_hash: Option<u64>,
}

struct PendingReset {
    new_context: String,
    msg: String,
    steering: bool,
}

impl ProviderAgentSession {
    pub async fn resume(
        id: String,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        thread_id: &str,
    ) -> Result<Arc<Self>, SessionError> {
        let (events, _) = broadcast::channel(512);
        let session = Arc::new(Self {
            id,
            provider,
            profile,
            instructions,
            inner: Mutex::new(SessionInner {
                status: SessionStatus::Idle,
                thread_id: Some(thread_id.to_owned()),
                current_turn_id: None,
                pending_reset: None,
                last_context_hash: None,
            }),
            events,
        });
        let mut notifications = session.provider.subscribe();
        let listener = Arc::downgrade(&session);
        tokio::spawn(async move {
            while let Ok(notification) = notifications.recv().await {
                let Some(session) = listener.upgrade() else { break };
                if session.handle_notification(notification).await {
                    break;
                }
            }
        });
        session
            .provider
            .resume_session(thread_id, &session.profile, &session.instructions)
            .await
            .map_err(map_provider_error)?;
        Ok(session)
    }

    pub fn provider_thread_id(&self) -> Option<String> {
        self.inner.try_lock().ok().and_then(|inner| inner.thread_id.clone())
    }

    pub async fn start(
        id: String,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        initial_context: Option<String>,
    ) -> Result<Arc<Self>, SessionError> {
        let (events, _) = broadcast::channel(512);
        let session = Arc::new(Self {
            id,
            provider,
            profile,
            instructions,
            inner: Mutex::new(SessionInner {
                status: SessionStatus::Idle,
                thread_id: None,
                current_turn_id: None,
                pending_reset: None,
                last_context_hash: None,
            }),
            events,
        });
        // Translate provider notifications into core SessionEvents.
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

        if let Some(context) = initial_context {
            session.send_user_msg(String::new(), false, Some(context)).await?;
        }
        Ok(session)
    }

    async fn handle_notification(self: &Arc<Self>, notification: ProviderNotification) -> bool {
        let mut inner = self.inner.lock().await;
        match notification {
            ProviderNotification::TurnStarted { thread_id, turn_id } => {
                if inner.thread_id.as_ref() == Some(&thread_id) {
                    inner.current_turn_id = Some(turn_id.clone());
                    inner.status = SessionStatus::Running;
                    let _ = self.events.send(SessionEvent::TurnStarted { turn_id });
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
                    let is_current = inner.current_turn_id.as_ref() == Some(&turn_id);
                    inner.current_turn_id = None;
                    inner.status = SessionStatus::Idle;
                    let _ = self
                        .events
                        .send(SessionEvent::TurnTerminal { turn_id: turn_id.clone(), outcome });
                    if let Some(reason) = error {
                        let _ = self.events.send(SessionEvent::Failed { reason });
                    }
                    if is_current {
                        if let Some(pending) = inner.pending_reset.take() {
                            drop(inner);
                            let context = pending.new_context;
                            let msg = pending.msg;
                            let steering = pending.steering;
                            let _ = self.send_user_msg(msg, steering, Some(context)).await;
                        }
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
            _ => false,
        }
    }

    fn context_hash(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

#[async_trait::async_trait]
impl AgentSession for ProviderAgentSession {
    fn id(&self) -> &str {
        &self.id
    }

    fn status(&self) -> SessionStatus {
        // Synchronous snapshot is best-effort; the authoritative signal is the
        // event stream.
        SessionStatus::Idle
    }

    fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    async fn send_user_msg(
        &self,
        msg: String,
        steering: bool,
        reset_context_to: Option<String>,
    ) -> Result<(), SessionError> {
        let mut inner = self.inner.lock().await;

        if inner.status == SessionStatus::Failed {
            return Err(SessionError::Unavailable);
        }

        let context_changed = reset_context_to.as_ref().map_or(false, |context| {
            let hash = Self::context_hash(context);
            let changed = inner.last_context_hash != Some(hash);
            changed
        });

        // If a turn is running and we need to replace context, fence the turn.
        if context_changed && inner.status == SessionStatus::Running {
            let context = reset_context_to.expect("context_changed implies Some");
            inner.pending_reset = Some(PendingReset { new_context: context, msg, steering });
            if let (Some(thread_id), Some(turn_id)) =
                (inner.thread_id.clone(), inner.current_turn_id.clone())
            {
                let _ = self.provider.interrupt(&thread_id, &turn_id).await;
            }
            return Ok(());
        }

        // If context changed while idle, replace the physical session.
        let need_new_session =
            inner.thread_id.is_none() || (context_changed && inner.status == SessionStatus::Idle);

        if need_new_session {
            let context = reset_context_to.as_ref();
            let provider_session = self
                .provider
                .start_session(&self.profile, &self.instructions)
                .await
                .map_err(map_provider_error)?;
            if let Some(context) = context {
                self.provider
                    .inject_context(&provider_session.thread_id, context)
                    .await
                    .map_err(map_provider_error)?;
                inner.last_context_hash = Some(Self::context_hash(context));
            }
            let old_id = inner.thread_id.replace(provider_session.thread_id.clone());
            if let Some(old_id) = old_id {
                let _ = self.events.send(SessionEvent::SessionReplaced {
                    old_id,
                    new_id: provider_session.thread_id.clone(),
                });
            }
        } else if context_changed {
            // Should have been handled above, but guard anyway.
            if let Some(context) = reset_context_to {
                if let Some(thread_id) = inner.thread_id.as_ref() {
                    self.provider
                        .inject_context(thread_id, &context)
                        .await
                        .map_err(map_provider_error)?;
                    inner.last_context_hash = Some(Self::context_hash(&context));
                }
            }
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
            // expected to emit them through the event queue debounce path so
            // they arrive when the session is idle.
        } else if !msg.is_empty() {
            let turn = self
                .provider
                .start_turn(&thread_id, &self.profile, &msg)
                .await
                .map_err(map_provider_error)?;
            inner.current_turn_id = Some(turn.turn_id.clone());
            inner.status = SessionStatus::Running;
            let _ = self.events.send(SessionEvent::TurnStarted { turn_id: turn.turn_id });
        }

        Ok(())
    }
}

fn map_provider_error(error: ProviderError) -> SessionError {
    SessionError::Failed(error.to_string())
}
