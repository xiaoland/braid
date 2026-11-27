use super::*;
use crate::provider::{ProviderSession, ProviderTurn};
use tokio::sync::{mpsc, oneshot};

type StartReply = oneshot::Sender<Result<ProviderTurn, ProviderError>>;

/// Only the external provider is controlled. The production adapter owns
/// notification ordering, deduplication, availability and stop confirmation.
struct ControlledProvider {
    notifications: broadcast::Sender<ProviderNotification>,
    starts: mpsc::Sender<StartReply>,
}

#[async_trait::async_trait]
impl AgentProvider for ControlledProvider {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.notifications.subscribe()
    }
    async fn closed(&self) {
        std::future::pending::<()>().await;
    }
    async fn start_session(&self, _: &Profile, _: &str) -> Result<ProviderSession, ProviderError> {
        Ok(ProviderSession { thread_id: "thread".into() })
    }
    async fn inject_context(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        unreachable!("this fixture does not materialize Context")
    }
    async fn resume_session(
        &self,
        _: &str,
        _: &Profile,
        _: &str,
    ) -> Result<ProviderSession, ProviderError> {
        unreachable!("this fixture does not resume")
    }
    async fn start_turn(
        &self,
        _: &str,
        _: &Profile,
        _: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        let (reply, response) = oneshot::channel();
        self.starts.send(reply).await.unwrap();
        response.await.unwrap()
    }
    async fn steer(&self, _: &str, _: &str, _: &str) -> Result<(), ProviderError> {
        unreachable!("this fixture does not steer")
    }
    async fn interrupt(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        Err(ProviderError::Disconnected)
    }
}

async fn fixture()
-> (Arc<ControlledProvider>, Arc<ProviderAgentSession>, mpsc::Receiver<StartReply>) {
    let (starts, requests) = mpsc::channel(1);
    let provider = Arc::new(ControlledProvider { notifications: broadcast::channel(32).0, starts });
    let config: crate::config::Config =
        toml::from_str(include_str!("../../config.example.toml")).unwrap();
    let session = ProviderAgentSession::start(
        provider.clone(),
        config.profiles[0].clone(),
        String::new(),
        None,
    )
    .await
    .unwrap();
    (provider, session, requests)
}

async fn start(
    session: &Arc<ProviderAgentSession>,
    requests: &mut mpsc::Receiver<StartReply>,
    turn: &str,
) {
    let sender = session.clone();
    let sending = tokio::spawn(async move { sender.send_user_msg("input".into(), false).await });
    requests.recv().await.unwrap().send(Ok(ProviderTurn { turn_id: turn.into() })).unwrap();
    assert_eq!(sending.await.unwrap().unwrap(), SendResult::Started);
}

async fn next(events: &mut broadcast::Receiver<SessionEvent>) -> SessionEvent {
    tokio::time::timeout(std::time::Duration::from_secs(2), events.recv())
        .await
        .expect("adapter lifecycle event did not arrive")
        .unwrap()
}

#[tokio::test]
async fn stale_and_duplicate_notifications_cannot_settle_the_next_turn() {
    let (provider, session, mut requests) = fixture().await;
    let mut events = session.events();
    start(&session, &mut requests, "first").await;
    assert!(
        matches!(next(&mut events).await, SessionEvent::TurnStarted { provider_turn_id } if provider_turn_id == "first")
    );
    let completed = ProviderNotification::TurnCompleted {
        thread_id: "thread".into(),
        turn_id: "first".into(),
        status: "completed".into(),
        error: None,
    };
    provider.notifications.send(completed.clone()).unwrap();
    assert!(
        matches!(next(&mut events).await, SessionEvent::TurnTerminal { provider_turn_id, outcome: TurnOutcome::Completed, error: None } if provider_turn_id == "first")
    );

    start(&session, &mut requests, "second").await;
    assert!(
        matches!(next(&mut events).await, SessionEvent::TurnStarted { provider_turn_id } if provider_turn_id == "second")
    );
    // The final current terminal is a stream barrier: all earlier frames have
    // been consumed when it arrives, so absence checks do not race the listener.
    provider.notifications.send(completed).unwrap();
    provider
        .notifications
        .send(ProviderNotification::TurnStarted {
            thread_id: "thread".into(),
            turn_id: "first".into(),
        })
        .unwrap();
    provider
        .notifications
        .send(ProviderNotification::TurnCompleted {
            thread_id: "another-thread".into(),
            turn_id: "second".into(),
            status: "completed".into(),
            error: None,
        })
        .unwrap();
    provider
        .notifications
        .send(ProviderNotification::TurnCompleted {
            thread_id: "thread".into(),
            turn_id: "second".into(),
            status: "failed".into(),
            error: Some("provider rejected the model".into()),
        })
        .unwrap();
    assert!(
        matches!(next(&mut events).await, SessionEvent::TurnTerminal { provider_turn_id, outcome: TurnOutcome::Failed, error: Some(error) } if provider_turn_id == "second" && error == "provider rejected the model")
    );
    assert!(matches!(events.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
    session.close().await.unwrap();
}

#[tokio::test]
async fn disconnect_latches_unavailability_and_does_not_prove_an_active_turn_stopped() {
    for active in [false, true] {
        let (provider, session, mut requests) = fixture().await;
        let mut events = session.events();
        if active {
            start(&session, &mut requests, "running").await;
            assert!(matches!(next(&mut events).await, SessionEvent::TurnStarted { .. }));
        }
        provider.notifications.send(ProviderNotification::Disconnected).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !session.is_unavailable() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if active {
            assert!(
                matches!(next(&mut events).await, SessionEvent::TurnTerminal { provider_turn_id, outcome: TurnOutcome::Unknown, error: Some(error) } if provider_turn_id == "running" && error == "provider disconnected")
            );
            let error = session.close().await.unwrap_err();
            assert!(
                error.to_string().contains("old execution has no confirmed terminal"),
                "{error}"
            );
        } else {
            session.close().await.unwrap();
        }
        assert_eq!(
            session.send_user_msg("later input".into(), false).await.unwrap(),
            SendResult::Deferred
        );
        assert!(matches!(events.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
    }
}
