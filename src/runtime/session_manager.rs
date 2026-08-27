use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    agent_session::AgentSession,
    config::Profile,
    provider::{AgentProvider, ProviderAgentSession},
};

/// In-process manager for the active Agent Sessions of this worker.
///
/// MVP: one session per physical provider thread id. The key is the current
/// `provider_session_id` (a.k.a. provider thread id). When a context reset
/// replaces the physical session, `replace` updates the key atomically.
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

    pub async fn insert(&self, provider_session_id: String, session: Arc<ProviderAgentSession>) {
        let mut sessions = self.sessions.lock().await;
        sessions.insert(provider_session_id, session);
    }

    pub async fn replace(&self, old_provider_session_id: &str, new_provider_session_id: String) {
        let mut sessions = self.sessions.lock().await;
        let session = sessions.remove(old_provider_session_id);
        if let Some(session) = session {
            sessions.insert(new_provider_session_id, session);
        }
    }

    pub async fn start(
        &self,
        provider_session_id: String,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        initial_context: Option<String>,
    ) -> Result<Arc<ProviderAgentSession>, anyhow::Error> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&provider_session_id) {
            return Ok(Arc::clone(session));
        }
        let session = ProviderAgentSession::start(
            provider_session_id.clone(),
            provider,
            profile,
            instructions,
            initial_context,
        )
        .await?;
        sessions.insert(provider_session_id, Arc::clone(&session));
        Ok(session)
    }

    pub async fn resume(
        &self,
        provider_session_id: String,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
    ) -> Result<Arc<ProviderAgentSession>, anyhow::Error> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(&provider_session_id) {
            return Ok(Arc::clone(session));
        }
        let session = ProviderAgentSession::resume(
            provider_session_id.clone(),
            provider,
            profile,
            instructions,
            &provider_session_id,
        )
        .await?;
        sessions.insert(provider_session_id, Arc::clone(&session));
        Ok(session)
    }

    pub async fn remove(&self, provider_session_id: &str) {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(provider_session_id);
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
