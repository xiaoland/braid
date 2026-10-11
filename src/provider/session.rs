#![allow(clippy::all, clippy::pedantic)]
use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};

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
    Starting,
    Running,
    Failed,
}

/// Adapter-level wrapper that exposes the core `AgentSession` contract over the
/// lower-level `AgentProvider` primitives.
pub struct ProviderAgentSession {
    provider: Arc<dyn AgentProvider>,
    unavailable: AtomicBool,
    listener: OnceLock<tokio::task::AbortHandle>,
    profile: Profile,
    instructions: String,
    commands: Mutex<()>,
    close_outcome: Mutex<Option<Result<(), SessionError>>>,
    inner: Mutex<SessionInner>,
    events: broadcast::Sender<SessionEvent>,
}

struct SessionInner {
    status: SessionStatus,
    stopped: bool,
    thread_id: Option<String>,
    current_turn_id: Option<String>,
    /// The most recently terminated turn. Guards against a provider
    /// re-emitting `TurnStarted` for a turn that already has its terminal,
    /// which would otherwise resurrect it (Started after Terminal).
    last_terminal_turn_id: Option<String>,
    early_terminal: Option<(String, String, Option<String>)>,
}

impl SessionInner {
    /// Record a turn start and report whether it is new. The provider reports
    /// one fact ("turn X started on this thread") through two observations —
    /// the `turn/start` response and the async notification — and the adapter
    /// must project exactly one `TurnStarted` per turn.
    fn note_turn_started(&mut self, turn_id: &str) -> bool {
        if self.status != SessionStatus::Starting
            || self.current_turn_id.is_some()
            || self.last_terminal_turn_id.as_deref() == Some(turn_id)
        {
            return false;
        }
        self.current_turn_id = Some(turn_id.to_owned());
        self.status = SessionStatus::Running;
        self.stopped = false;
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
            unavailable: AtomicBool::new(false),
            listener: OnceLock::new(),
            profile,
            instructions,
            commands: Mutex::new(()),
            close_outcome: Mutex::new(None),
            inner: Mutex::new(SessionInner {
                status: SessionStatus::Idle,
                stopped: true,
                thread_id,
                current_turn_id: None,
                last_terminal_turn_id: None,
                early_terminal: None,
            }),
            events,
        });
        let listener = Arc::downgrade(&session);
        let mut notifications = session.provider.subscribe();
        let connection = Arc::clone(&session.provider);
        let task = tokio::spawn(async move {
            loop {
                let notification = tokio::select! {
                    biased;
                    result = notifications.recv() => match result {
                        Ok(notification) => notification,
                        Err(error) => {
                            tracing::warn!(%error, "session notification stream lost; handle is unavailable");
                            ProviderNotification::Disconnected
                        }
                    },
                    () = connection.closed() => ProviderNotification::Disconnected,
                };
                let Some(session) = listener.upgrade() else { break };
                if session.handle_notification(notification).await {
                    break;
                }
            }
        });
        session.listener.set(task.abort_handle()).expect("session listener initialized once");
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

    fn finish_turn(
        &self,
        inner: &mut SessionInner,
        turn_id: String,
        status: &str,
        error: Option<String>,
    ) {
        let outcome = match status {
            "completed" => TurnOutcome::Completed,
            "interrupted" => TurnOutcome::Interrupted,
            "failed" => TurnOutcome::Failed,
            _ => TurnOutcome::Unknown,
        };
        inner.current_turn_id = None;
        inner.last_terminal_turn_id = Some(turn_id.clone());
        inner.stopped = outcome != TurnOutcome::Unknown;
        inner.status = if inner.stopped { SessionStatus::Idle } else { SessionStatus::Failed };
        if !inner.stopped {
            self.unavailable.store(true, Ordering::SeqCst);
        }
        if let Some(thread) = inner.thread_id.as_deref() {
            crate::telemetry::lifecycle_event(
                thread,
                Some(&turn_id),
                "terminal",
                status,
                error.as_deref(),
            );
        }
        let _ = self.events.send(SessionEvent::TurnTerminal {
            provider_turn_id: turn_id,
            outcome,
            error,
        });
    }

    async fn handle_notification(self: &Arc<Self>, notification: ProviderNotification) -> bool {
        let mut inner = self.inner.lock().await;
        match notification {
            ProviderNotification::TurnStarted { thread_id, turn_id } => {
                if inner.thread_id.as_ref() == Some(&thread_id) && inner.note_turn_started(&turn_id)
                {
                    crate::telemetry::bind_provider_turn(&thread_id, &turn_id);
                    crate::telemetry::lifecycle_event(
                        &thread_id,
                        Some(&turn_id),
                        "prompt",
                        "accepted",
                        None,
                    );
                    let _ = self
                        .events
                        .send(SessionEvent::TurnStarted { provider_turn_id: turn_id.clone() });
                    if let Some((ended, status, error)) = inner.early_terminal.take()
                        && ended == turn_id
                    {
                        self.finish_turn(&mut inner, ended, &status, error);
                    }
                }
                false
            }
            ProviderNotification::TurnCompleted { thread_id, turn_id, status, error } => {
                if inner.thread_id.as_ref() == Some(&thread_id) {
                    if inner.status == SessionStatus::Starting && inner.current_turn_id.is_none() {
                        // A terminal cannot establish the identity of a new start.
                        // Retain it until the response or Started confirms that identity.
                        if inner.last_terminal_turn_id.as_deref() != Some(&turn_id) {
                            inner.early_terminal = Some((turn_id, status, error));
                        }
                    } else if inner.current_turn_id.as_deref() == Some(turn_id.as_str()) {
                        self.finish_turn(&mut inner, turn_id, &status, error);
                    } else {
                        // Duplicate or stale terminal (e.g. replayed after
                        // resume): accepting it would double-emit the
                        // terminal and, worse, clear the tracking of a
                        // different turn that is currently running.
                        tracing::warn!(
                            %turn_id,
                            current = ?inner.current_turn_id,
                            "ignoring duplicate or stale TurnCompleted"
                        );
                    }
                }
                false
            }
            ProviderNotification::Disconnected => {
                // A started turn must never be left without a terminal:
                // synthesize Unknown for the in-flight turn, then fail the
                // session. Availability remains observable even without a turn
                // subscription; the group can recover this handle while idle.
                if let Some(turn_id) = inner.current_turn_id.clone() {
                    if let Some(thread) = inner.thread_id.as_deref() {
                        crate::telemetry::lifecycle_event(
                            thread,
                            Some(&turn_id),
                            "terminal",
                            "unknown",
                            Some("provider disconnected"),
                        );
                    }
                    inner.last_terminal_turn_id = Some(turn_id.clone());
                    let _ = self.events.send(SessionEvent::TurnTerminal {
                        provider_turn_id: turn_id,
                        outcome: TurnOutcome::Unknown,
                        error: Some("provider disconnected".into()),
                    });
                }
                inner.status = SessionStatus::Failed;
                self.unavailable.store(true, Ordering::SeqCst);
                true
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentSession for ProviderAgentSession {
    fn events(&self) -> broadcast::Receiver<SessionEvent> {
        self.events.subscribe()
    }

    fn is_unavailable(&self) -> bool {
        self.unavailable.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<(), SessionError> {
        self.unavailable.store(true, Ordering::SeqCst);
        let mut cached = self.close_outcome.lock().await;
        if let Some(result) = cached.as_ref() {
            return result.clone();
        }
        let _command = self.commands.lock().await;
        let mut events = self.events.subscribe();
        let (thread, turn) = {
            let inner = self.inner.lock().await;
            (inner.thread_id.clone(), inner.current_turn_id.clone())
        };
        let result = async {
            let thread = thread
                .clone()
                .ok_or_else(|| SessionError::Failed("no provider session to close".into()))?;
            let mut interruption_error = None;
            if let Some(turn) = turn.as_ref() {
                let interruption = self.provider.interrupt(&thread, &turn).await;
                interruption_error = interruption.as_ref().err().map(ToString::to_string);
                if interruption.is_ok() && !self.inner.lock().await.stopped {
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
                        loop {
                            let inner = self.inner.lock().await;
                            if inner.stopped || inner.status == SessionStatus::Failed {
                                break;
                            }
                            drop(inner);
                            if events.recv().await.is_err() {
                                break;
                            }
                        }
                    })
                    .await;
                }
            }
            let stopped = self.inner.lock().await.stopped;
            self.provider.close_session(&thread, stopped).await.map_err(|error| {
                match interruption_error {
                    Some(cause) => SessionError::Failed(format!("{error}; interrupt: {cause}")),
                    None => map_provider_error(error),
                }
            })
        }
        .await;
        if let Some(thread) = thread.as_deref() {
            crate::telemetry::lifecycle_event(
                thread,
                turn.as_deref(),
                "stop",
                if result.is_ok() { "confirmed" } else { "unconfirmed" },
                result.as_ref().err().map(ToString::to_string).as_deref(),
            );
        }
        if result.is_ok() {
            if let Some(listener) = self.listener.get() {
                listener.abort();
            }
        }
        *cached = Some(result.clone());
        result
    }

    async fn send_user_msg(&self, msg: String, steering: bool) -> Result<SendResult, SessionError> {
        if steering {
            let turn = self.inner.lock().await.current_turn_id.clone();
            return match turn {
                Some(turn) => self.steer_user_msg(msg, &turn).await,
                None => Ok(SendResult::Deferred),
            };
        }
        let _command = self.commands.lock().await;
        let (thread, previous_terminal) = {
            let mut inner = self.inner.lock().await;
            if self.is_unavailable() || inner.status != SessionStatus::Idle {
                if let Some(thread) = inner.thread_id.as_deref() {
                    crate::telemetry::lifecycle_event(
                        thread,
                        None,
                        "prompt",
                        "deferred",
                        Some("session is unavailable or not idle"),
                    );
                }
                return Ok(SendResult::Deferred);
            }
            if msg.is_empty() {
                return Ok(SendResult::Acknowledged);
            }
            let thread = inner
                .thread_id
                .clone()
                .ok_or_else(|| SessionError::Failed("no provider session".into()))?;
            inner.status = SessionStatus::Starting;
            inner.stopped = false;
            inner.early_terminal = None;
            (thread, inner.last_terminal_turn_id.clone())
        };
        let result = self.provider.start_turn(&thread, &self.profile, &msg).await;
        let mut inner = self.inner.lock().await;
        // Lifecycle notifications can arrive before the RPC response. They
        // remain authoritative, even when the late response fails.
        let observed =
            inner.current_turn_id.is_some() || inner.last_terminal_turn_id != previous_terminal;
        match result {
            Ok(turn) => {
                crate::telemetry::bind_provider_turn(&thread, &turn.turn_id);
                if inner.current_turn_id.is_none()
                    && inner.early_terminal.as_ref().is_some_and(|(id, _, _)| id == &turn.turn_id)
                {
                    inner.status = SessionStatus::Starting;
                }
                if inner.status == SessionStatus::Failed && !observed {
                    return Err(SessionError::Unknown(
                        "connection lost before start was confirmed".into(),
                    ));
                }
                if inner.note_turn_started(&turn.turn_id) {
                    crate::telemetry::lifecycle_event(
                        &thread,
                        Some(&turn.turn_id),
                        "prompt",
                        "accepted",
                        None,
                    );
                    let _ = self
                        .events
                        .send(SessionEvent::TurnStarted { provider_turn_id: turn.turn_id.clone() });
                }
                if let Some((ended, status, error)) = inner.early_terminal.take()
                    && ended == turn.turn_id
                    && inner.current_turn_id.as_deref() == Some(&ended)
                {
                    self.finish_turn(&mut inner, ended, &status, error);
                }
                Ok(SendResult::Started)
            }
            Err(error) if observed => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    inner.current_turn_id.as_deref().or(inner.last_terminal_turn_id.as_deref()),
                    "prompt_response",
                    "late_error",
                    Some(&format!("native lifecycle confirmed despite late response: {error}")),
                );
                Ok(SendResult::Started)
            }
            Err(ProviderError::Deferred(reason)) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    None,
                    "prompt",
                    "deferred",
                    Some(&reason),
                );
                inner.status = SessionStatus::Idle;
                inner.stopped = true;
                Ok(SendResult::Deferred)
            }
            Err(error @ ProviderError::Protocol(_)) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    None,
                    "prompt",
                    "failed",
                    Some(&error.to_string()),
                );
                inner.status = SessionStatus::Idle;
                inner.stopped = true;
                Err(map_provider_error(error))
            }
            Err(error) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    None,
                    "prompt",
                    "unknown",
                    Some(&error.to_string()),
                );
                inner.status = SessionStatus::Failed;
                self.unavailable.store(true, Ordering::SeqCst);
                Err(map_provider_error(error))
            }
        }
    }

    async fn steer_user_msg(
        &self,
        msg: String,
        expected_turn_id: &str,
    ) -> Result<SendResult, SessionError> {
        let _command = self.commands.lock().await;
        let thread = {
            let inner = self.inner.lock().await;
            if self.is_unavailable()
                || inner.status != SessionStatus::Running
                || inner.current_turn_id.as_deref() != Some(expected_turn_id)
            {
                if let Some(thread) = inner.thread_id.as_deref() {
                    crate::telemetry::lifecycle_event(
                        thread,
                        Some(expected_turn_id),
                        "steer",
                        "deferred",
                        Some("expected active turn is unavailable"),
                    );
                }
                return Ok(SendResult::Deferred);
            }
            inner
                .thread_id
                .clone()
                .ok_or_else(|| SessionError::Failed("no provider session".into()))?
        };
        match self.provider.steer(&thread, expected_turn_id, &msg).await {
            Ok(()) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    Some(expected_turn_id),
                    "steer",
                    "accepted",
                    None,
                );
                Ok(SendResult::Acknowledged)
            }
            Err(ProviderError::Deferred(reason)) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    Some(expected_turn_id),
                    "steer",
                    "deferred",
                    Some(&reason),
                );
                Ok(SendResult::Deferred)
            }
            Err(ProviderError::Protocol(reason)) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    Some(expected_turn_id),
                    "steer",
                    "deferred",
                    Some(&reason),
                );
                tracing::warn!(%reason, %expected_turn_id, "active turn refused steering; retain input for next turn");
                Ok(SendResult::Deferred)
            }
            Err(error) => {
                crate::telemetry::lifecycle_event(
                    &thread,
                    Some(expected_turn_id),
                    "steer",
                    "unknown",
                    Some(&error.to_string()),
                );
                Err(map_provider_error(error))
            }
        }
    }

    async fn interrupt(&self) -> Result<(), SessionError> {
        let _command = self.commands.lock().await;
        let (thread, turn) = {
            let inner = self.inner.lock().await;
            (inner.thread_id.clone(), inner.current_turn_id.clone())
        };
        match (thread, turn) {
            (Some(thread), Some(turn)) => {
                self.provider.interrupt(&thread, &turn).await.map_err(map_provider_error)
            }
            _ => Ok(()),
        }
    }
}

pub(super) fn map_provider_error(error: ProviderError) -> SessionError {
    match error {
        ProviderError::Protocol(message) | ProviderError::Deferred(message) => {
            SessionError::Failed(message)
        }
        error => SessionError::Unknown(error.to_string()),
    }
}

impl Drop for ProviderAgentSession {
    fn drop(&mut self) {
        if let Some(listener) = self.listener.get() {
            listener.abort();
        }
    }
}
