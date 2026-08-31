#![allow(clippy::wildcard_imports)]
use std::sync::Arc;

use super::*;

pub(crate) fn required_string(
    value: &Value,
    field: &str,
    context: &str,
) -> Result<String, ProviderError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ProviderError::Protocol(format!("{context} omitted {field}")))
}

pub(crate) fn path_text(path: &Path) -> Result<&str, ProviderError> {
    path.to_str().ok_or_else(|| {
        ProviderError::Protocol(format!("provider workspace is not UTF-8: {}", path.display()))
    })
}

#[async_trait::async_trait]
impl AgentProvider for Arc<dyn AgentProvider> {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.as_ref().subscribe()
    }

    async fn closed(&self) {
        self.as_ref().closed().await;
    }

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        self.as_ref().start_session(profile, developer_instructions).await
    }

    async fn inject_context(&self, thread_id: &str, context: &str) -> Result<(), ProviderError> {
        self.as_ref().inject_context(thread_id, context).await
    }

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        self.as_ref().resume_session(thread_id, profile, developer_instructions).await
    }

    async fn start_turn(
        &self,
        thread_id: &str,
        profile: &Profile,
        event_references: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        self.as_ref().start_turn(thread_id, profile, event_references).await
    }

    async fn steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError> {
        self.as_ref().steer(thread_id, expected_turn_id, event_references).await
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), ProviderError> {
        self.as_ref().interrupt(thread_id, turn_id).await
    }
}

#[async_trait::async_trait]
impl AgentProvider for Box<dyn AgentProvider> {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.as_ref().subscribe()
    }

    async fn closed(&self) {
        self.as_ref().closed().await;
    }

    async fn start_session(
        &self,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        self.as_ref().start_session(profile, developer_instructions).await
    }

    async fn inject_context(&self, thread_id: &str, context: &str) -> Result<(), ProviderError> {
        self.as_ref().inject_context(thread_id, context).await
    }

    async fn resume_session(
        &self,
        thread_id: &str,
        profile: &Profile,
        developer_instructions: &str,
    ) -> Result<ProviderSession, ProviderError> {
        self.as_ref().resume_session(thread_id, profile, developer_instructions).await
    }

    async fn start_turn(
        &self,
        thread_id: &str,
        profile: &Profile,
        event_references: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        self.as_ref().start_turn(thread_id, profile, event_references).await
    }

    async fn steer(
        &self,
        thread_id: &str,
        expected_turn_id: &str,
        event_references: &str,
    ) -> Result<(), ProviderError> {
        self.as_ref().steer(thread_id, expected_turn_id, event_references).await
    }

    async fn interrupt(&self, thread_id: &str, turn_id: &str) -> Result<(), ProviderError> {
        self.as_ref().interrupt(thread_id, turn_id).await
    }
}

pub async fn connect_provider(
    config: &crate::config::ProviderConfig,
) -> Result<Arc<dyn crate::provider::AgentProvider>, crate::provider::ProviderError> {
    if let Some(codex) = &config.codex {
        let provider = CodexProvider::connect(codex).await?;
        Ok(Arc::new(provider))
    } else if let Some(pi) = &config.pi {
        let provider = PiProvider::connect(pi);
        Ok(Arc::new(provider))
    } else {
        Err(crate::provider::ProviderError::Protocol("no provider configured".into()))
    }
}
