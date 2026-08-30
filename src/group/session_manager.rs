use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    agent_session::{AgentSession, SessionError},
    config::Profile,
    provider::{AgentProvider, ProviderAgentSession},
};

/// In-process manager for the active Agent Sessions of one connection epoch.
///
/// The durable store is the authority for session identity; this map is an
/// ephemeral cache keyed by the current provider thread id. Because sessions
/// bind the epoch's provider handle, workers build a fresh manager per
/// connection epoch and repopulate it from the store via `resume`.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<ProviderAgentSession>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self { sessions: Mutex::new(HashMap::new()) }
    }

    pub async fn get(&self, provider_session_id: &str) -> Option<Arc<dyn AgentSession>> {
        let sessions = self.sessions.lock().await;
        sessions.get(provider_session_id).map(|s| Arc::clone(s) as Arc<dyn AgentSession>)
    }

    /// Start a fresh Agent Session with an initial materialized context.
    ///
    /// The adapter owns physical session creation and context injection; the
    /// manager keys the session by the thread id the adapter actually created.
    pub async fn start(
        &self,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        initial_context: String,
    ) -> Result<Arc<ProviderAgentSession>, SessionError> {
        let session =
            ProviderAgentSession::start(provider, profile, instructions, Some(initial_context))
                .await?;
        let thread_id = session
            .thread_id()
            .await
            .ok_or_else(|| SessionError::Failed("AgentSession has no provider thread".into()))?;
        let mut sessions = self.sessions.lock().await;
        if let Some(existing) = sessions.get(&thread_id) {
            return Ok(Arc::clone(existing));
        }
        sessions.insert(thread_id, Arc::clone(&session));
        Ok(session)
    }

    pub async fn resume(
        &self,
        provider_session_id: String,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
    ) -> Result<Arc<ProviderAgentSession>, SessionError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&provider_session_id) {
            return Ok(Arc::clone(session));
        }
        let session =
            ProviderAgentSession::resume(provider, profile, instructions, &provider_session_id)
                .await?;
        sessions.insert(provider_session_id, Arc::clone(&session));
        Ok(session)
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
