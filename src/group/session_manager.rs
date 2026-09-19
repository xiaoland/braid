use crate::{
    agent_session::{AgentSession, CreatedSession, SessionError, SessionFactory},
    config::Profile,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::sync::Mutex;

/// Ephemeral handles indexed by the store's opaque session identity. Physical
/// resource ownership and sharing stay in the injected adapter factory.
pub(super) struct SessionManager {
    factory: Arc<dyn SessionFactory>,
    sessions: Mutex<HashMap<String, Arc<dyn AgentSession>>>,
}

impl SessionManager {
    pub(super) fn new(factory: Arc<dyn SessionFactory>) -> Self {
        Self { factory, sessions: Mutex::new(HashMap::new()) }
    }

    pub(super) async fn check(&self) -> Result<(), SessionError> {
        self.factory.check().await
    }

    pub(super) async fn get(&self, id: &str) -> Option<Arc<dyn AgentSession>> {
        self.sessions.lock().await.get(id).cloned()
    }

    pub(super) async fn is_live(&self, id: &str) -> bool {
        self.get(id).await.is_some_and(|session| !session.is_unavailable())
    }

    pub(super) async fn live_ids(&self) -> Vec<String> {
        self.sessions
            .lock()
            .await
            .iter()
            .filter(|(_, session)| !session.is_unavailable())
            .map(|(id, _)| id.clone())
            .collect()
    }

    pub(super) async fn start(
        &self,
        profile: Profile,
        instructions: String,
        context: String,
    ) -> Result<String, SessionError> {
        let CreatedSession { id, session } =
            self.factory.start(profile, instructions, context).await?;
        self.sessions.lock().await.insert(id.clone(), session);
        Ok(id)
    }

    pub(super) async fn resume(
        &self,
        id: String,
        profile: Profile,
        instructions: String,
    ) -> Result<(), SessionError> {
        if self.is_live(&id).await {
            return Ok(());
        }
        self.remove(&id).await;
        let created = self.factory.resume(&id, profile, instructions).await?;
        if created.id != id {
            let _ = created.session.close().await;
            return Err(SessionError::Failed("resume changed the durable session identity".into()));
        }
        self.sessions.lock().await.insert(id, created.session);
        Ok(())
    }

    pub(super) async fn remove(&self, id: &str) {
        let removed = self.sessions.lock().await.remove(id);
        if let Some(session) = removed
            && let Err(error) = session.close().await
        {
            tracing::debug!(%error, provider_session = id, "session release could not interrupt a turn");
        }
    }

    pub(super) async fn retain(&self, ids: &HashSet<String>) {
        let obsolete: Vec<_> =
            self.sessions.lock().await.keys().filter(|id| !ids.contains(*id)).cloned().collect();
        for id in obsolete {
            self.remove(&id).await;
        }
    }
}
