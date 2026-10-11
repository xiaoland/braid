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
    stopped: Mutex<HashSet<String>>,
}

impl SessionManager {
    pub(super) fn new(factory: Arc<dyn SessionFactory>) -> Self {
        Self { factory, sessions: Mutex::new(HashMap::new()), stopped: Mutex::new(HashSet::new()) }
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
        self.stopped.lock().await.remove(&id);
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
        self.remove(&id).await?;
        let created = self.factory.resume(&id, profile, instructions).await?;
        if created.id != id {
            let unexpected = created.id;
            self.sessions.lock().await.insert(unexpected.clone(), created.session);
            let reason = match self.remove(&unexpected).await {
                Ok(()) => "resume changed the durable session identity".into(),
                Err(error) => format!(
                    "resume changed the durable session identity; unexpected session stop failed: {error}"
                ),
            };
            return Err(SessionError::Failed(reason));
        }
        self.sessions.lock().await.insert(id, created.session);
        self.stopped.lock().await.remove(&created.id);
        Ok(())
    }

    pub(super) async fn remove(&self, id: &str) -> Result<(), SessionError> {
        if let Some(session) = self.get(id).await {
            // Keep the handle until its adapter proves the old execution stopped.
            session.close().await?;
            self.sessions.lock().await.remove(id);
            self.stopped.lock().await.insert(id.to_owned());
        }
        Ok(())
    }

    pub(super) async fn confirmed_stopped(&self, id: &str) -> bool {
        self.stopped.lock().await.contains(id)
    }

    pub(super) async fn retain(&self, ids: &HashSet<String>) -> Result<(), SessionError> {
        let obsolete: Vec<_> =
            self.sessions.lock().await.keys().filter(|id| !ids.contains(*id)).cloned().collect();
        let mut failure = None;
        for id in obsolete {
            if let Err(error) = self.remove(&id).await {
                tracing::error!(%error, provider_session = %id, "execution stop remains unconfirmed");
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
