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
        which::which(&self.config.executable).map_err(|_| SessionError::Unavailable)?;
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::agent_session::{SendResult, SessionEvent, TurnOutcome};
    use std::{os::unix::fs::PermissionsExt, path::PathBuf};
    use tokio::time::{Duration, timeout};

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Exercises real child ownership and RPC routing, without an LLM or network.
    /// This is adapter evidence, not product acceptance.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn pi_sessions_isolate_context_failure_and_release() {
        let root = Fixture(std::env::temp_dir().join(format!("braid-pi-{}", uuid::Uuid::now_v7())));
        std::fs::create_dir_all(&root.0).unwrap();
        let executable = root.0.join("pi");
        std::fs::write(&executable, r#"#!/bin/sh
while IFS= read -r frame; do
    id=$(printf '%s' "$frame" | sed -E 's/.*"id":([0-9]+).*/\1/')
    printf '%s\n' "$frame" >> "$PWD/requests"
    case "$frame" in
        *'"type":"get_state"'*) printf '{"id":%s,"success":true,"data":{"sessionFile":"%s"}}\n' "$id" "$$" ;;
        *) printf '{"id":%s,"success":true}\n' "$id" ;;
    esac
    case "$frame" in
        *complete-and-exit*) printf '{"type":"agent_settled"}\n'; exit 0 ;;
    esac
done
"#).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let factory = PiSessions {
            config: PiConfig {
                executable,
                provider: None,
                model: None,
                api_key_environment: None,
                api_key_file: None,
                thinking: None,
                home: None,
            },
        };
        let config: Config = toml::from_str(include_str!("../../config.example.toml")).unwrap();
        let mut first_profile = config.profiles[0].clone();
        first_profile.workspace = Some(root.0.join("first"));
        let mut second_profile = first_profile.clone();
        second_profile.workspace = Some(root.0.join("second"));
        std::fs::create_dir_all(first_profile.workspace()).unwrap();
        std::fs::create_dir_all(second_profile.workspace()).unwrap();
        let first = factory
            .start(first_profile.clone(), "first instructions".into(), "first context".into())
            .await
            .unwrap();
        let second = factory
            .start(second_profile.clone(), "second instructions".into(), "second context".into())
            .await
            .unwrap();
        assert_ne!(first.id, second.id);
        let mut first_events = first.session.events();
        assert!(matches!(
            first.session.send_user_msg("first input".into(), false).await.unwrap(),
            SendResult::Started
        ));
        assert!(matches!(first_events.recv().await.unwrap(), SessionEvent::TurnStarted { .. }));
        assert!(
            std::process::Command::new("kill")
                .args(["-KILL", &first.id])
                .status()
                .unwrap()
                .success()
        );
        assert!(matches!(
            timeout(Duration::from_secs(5), first_events.recv()).await.unwrap().unwrap(),
            SessionEvent::TurnTerminal { outcome: TurnOutcome::Unknown, .. }
        ));
        assert!(first.session.is_unavailable());
        assert!(!second.session.is_unavailable());
        assert!(matches!(
            second.session.send_user_msg("second input".into(), false).await.unwrap(),
            SendResult::Started
        ));
        let first_requests =
            std::fs::read_to_string(first_profile.workspace().join("requests")).unwrap();
        let second_requests =
            std::fs::read_to_string(second_profile.workspace().join("requests")).unwrap();
        assert!(first_requests.contains("first context"));
        assert!(!first_requests.contains("second context"));
        assert!(second_requests.contains("second context"));
        assert!(!second_requests.contains("first context"));
        let idle =
            factory.start(first_profile.clone(), String::new(), String::new()).await.unwrap();
        assert!(
            std::process::Command::new("kill")
                .args(["-KILL", &idle.id])
                .status()
                .unwrap()
                .success()
        );
        timeout(Duration::from_secs(5), async {
            while !idle.session.is_unavailable() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("idle failure is latched without a turn subscriber");
        let mut late_events = idle.session.events();
        assert!(late_events.try_recv().is_err());
        assert!(matches!(
            idle.session.send_user_msg("late".into(), false).await,
            Err(SessionError::Unavailable)
        ));
        drop(idle);
        let finished =
            factory.start(first_profile.clone(), String::new(), String::new()).await.unwrap();
        let mut events = finished.session.events();
        finished.session.send_user_msg("complete-and-exit".into(), false).await.unwrap();
        assert!(matches!(events.recv().await.unwrap(), SessionEvent::TurnStarted { .. }));
        assert!(matches!(
            timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap(),
            SessionEvent::TurnTerminal { outcome: TurnOutcome::Completed, .. }
        ));
        drop(finished);
        first.session.close().await.ok();
        drop(first);
        // Releasing one handle neither stops nor replaces the surviving session.
        assert!(matches!(
            second.session.send_user_msg("steer".into(), true).await.unwrap(),
            SendResult::Acknowledged
        ));
        let second_pid = second.id.clone();
        second.session.close().await.unwrap();
        drop(second);
        timeout(Duration::from_secs(5), async {
            loop {
                let alive = std::process::Command::new("kill")
                    .args(["-0", &second_pid])
                    .stderr(std::process::Stdio::null())
                    .status()
                    .unwrap()
                    .success();
                if !alive {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("released Pi child must exit");
    }
}
