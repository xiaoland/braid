use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

use super::{AgentProvider, CodexProvider, PiProvider, ProviderAgentSession};
use crate::{
    agent_session::{CreatedSession, SessionError, SessionFactory},
    config::{CodexConfig, Config, PiConfig, Profile},
};

/// Provider selection happens at composition; neither groups nor session
/// indices choose physical processes. Runtime entries are unique per adapter.
pub(crate) fn session_factories(
    config: &Config,
) -> anyhow::Result<HashMap<String, Arc<dyn SessionFactory>>> {
    let mut factories = HashMap::new();
    let mut codex: Option<Arc<dyn SessionFactory>> = None;
    for profile in &config.profiles {
        let settings = config.provider_config_for_profile(profile)?;
        let factory: Arc<dyn SessionFactory> = if let Some(config) = settings.codex {
            Arc::clone(codex.get_or_insert_with(|| {
                Arc::new(CodexSessions { config, connection: Mutex::new(None) })
            }))
        } else if let Some(config) = settings.pi {
            Arc::new(PiSessions { config })
        } else {
            anyhow::bail!("Profile {} has no session adapter", profile.id);
        };
        factories.insert(profile.id.clone(), factory);
    }
    Ok(factories)
}

struct CodexSessions {
    config: CodexConfig,
    connection: Mutex<Option<Arc<CodexProvider>>>,
}

impl CodexSessions {
    async fn connection(&self) -> Result<Arc<dyn AgentProvider>, SessionError> {
        let mut cached = self.connection.lock().await;
        if cached.as_ref().is_none_or(|provider| provider.is_closed()) {
            *cached = Some(Arc::new(
                CodexProvider::connect(&self.config)
                    .await
                    .map_err(super::session::map_provider_error)?,
            ));
        }
        Ok(Arc::clone(cached.as_ref().expect("connected provider")) as Arc<dyn AgentProvider>)
    }
}

#[async_trait::async_trait]
impl SessionFactory for CodexSessions {
    async fn check(&self) -> Result<(), SessionError> {
        self.connection().await.map(|_| ())
    }

    async fn start(
        &self,
        profile: Profile,
        instructions: String,
        context: String,
    ) -> Result<CreatedSession, SessionError> {
        let session = ProviderAgentSession::start(
            self.connection().await?,
            profile,
            instructions,
            Some(context),
        )
        .await?;
        created(session).await
    }

    async fn resume(
        &self,
        id: &str,
        profile: Profile,
        instructions: String,
    ) -> Result<CreatedSession, SessionError> {
        let session =
            ProviderAgentSession::resume(self.connection().await?, profile, instructions, id)
                .await?;
        created(session).await
    }
}

struct PiSessions {
    config: PiConfig,
}

#[async_trait::async_trait]
impl SessionFactory for PiSessions {
    async fn check(&self) -> Result<(), SessionError> {
        // Pi has no global process: processes belong to physical sessions.
        // Readiness is periodic; full material identity is verified before each physical spawn.
        which::which(&self.config.executable).map_err(|error| {
            SessionError::Unavailable(format!(
                "Pi executable {}: {error}",
                self.config.executable.display()
            ))
        })?;
        self.config.api_key().map(|_| ()).map_err(|error| SessionError::Failed(error.to_string()))
    }

    async fn start(
        &self,
        profile: Profile,
        instructions: String,
        context: String,
    ) -> Result<CreatedSession, SessionError> {
        let provider = Arc::new(PiProvider::connect(&self.config));
        let session =
            ProviderAgentSession::start(provider, profile, instructions, Some(context)).await?;
        created(session).await
    }

    async fn resume(
        &self,
        id: &str,
        profile: Profile,
        instructions: String,
    ) -> Result<CreatedSession, SessionError> {
        let provider = Arc::new(PiProvider::connect(&self.config));
        let session = ProviderAgentSession::resume(provider, profile, instructions, id).await?;
        created(session).await
    }
}

async fn created(session: Arc<ProviderAgentSession>) -> Result<CreatedSession, SessionError> {
    let id = session
        .thread_id()
        .await
        .ok_or_else(|| SessionError::Failed("adapter returned no session identity".into()))?;
    Ok(CreatedSession { id, session })
}
