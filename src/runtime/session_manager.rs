use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;

use crate::{
    agent_session::AgentSession,
    config::Profile,
    provider::{AgentProvider, ProviderAgentSession},
};

/// In-process manager for the active Agent Sessions of this worker.
/// MVP: one session per work-item assignment; retrieved by assignment id.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<ProviderAgentSession>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self { sessions: Mutex::new(HashMap::new()) }
    }

    pub async fn get_or_start(
        &self,
        assignment_id: &str,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        initial_context: Option<String>,
    ) -> Result<Arc<dyn AgentSession>, anyhow::Error> {
        let mut sessions = self.sessions.lock().await;
        if let Some(session) = sessions.get(assignment_id) {
            return Ok(Arc::clone(session) as Arc<dyn AgentSession>);
        }
        let session = ProviderAgentSession::start(
            assignment_id.to_owned(),
            provider,
            profile,
            instructions,
            initial_context,
        )
        .await?;
        sessions.insert(assignment_id.to_owned(), Arc::clone(&session));
        Ok(session as Arc<dyn AgentSession>)
    }

    pub async fn resume(
        &self,
        assignment_id: &str,
        provider: Arc<dyn AgentProvider>,
        profile: Profile,
        instructions: String,
        thread_id: &str,
    ) -> Result<Arc<dyn AgentSession>, anyhow::Error> {
        let mut sessions = self.sessions.lock().await;
        let session = ProviderAgentSession::resume(
            assignment_id.to_owned(),
            provider,
            profile,
            instructions,
            thread_id,
        )
        .await?;
        sessions.insert(assignment_id.to_owned(), Arc::clone(&session));
        Ok(session as Arc<dyn AgentSession>)
    }

    pub async fn remove(&self, assignment_id: &str) {
        let mut sessions = self.sessions.lock().await;
        sessions.remove(assignment_id);
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
