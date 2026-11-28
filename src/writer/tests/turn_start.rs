//! Real provider adapter + dispatch start boundary + production publication source resolver.
use super::*;
use crate::{
    agent_session::{AgentSession, SessionError, SessionEvent, TurnOutcome},
    provider::{
        AgentProvider, ProviderAgentSession, ProviderError, ProviderNotification, ProviderSession,
        ProviderTurn,
    },
};
use tokio::sync::{Notify, broadcast};

#[derive(Clone, Copy)]
enum Reply {
    Success,
    LateError,
    Deferred,
    Unknown,
}
struct DelayedProvider {
    notifications: broadcast::Sender<ProviderNotification>,
    release: Notify,
    reply: Reply,
}
impl DelayedProvider {
    fn new(reply: Reply) -> Arc<Self> {
        Arc::new(Self { notifications: broadcast::channel(32).0, release: Notify::new(), reply })
    }
}
#[async_trait::async_trait]
impl AgentProvider for DelayedProvider {
    fn subscribe(&self) -> broadcast::Receiver<ProviderNotification> {
        self.notifications.subscribe()
    }
    async fn closed(&self) {
        std::future::pending::<()>().await;
    }
    async fn start_session(&self, _: &Profile, _: &str) -> Result<ProviderSession, ProviderError> {
        Ok(ProviderSession { thread_id: "provider".into() })
    }
    async fn inject_context(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn resume_session(
        &self,
        _: &str,
        _: &Profile,
        _: &str,
    ) -> Result<ProviderSession, ProviderError> {
        unreachable!("test never resumes")
    }
    async fn start_turn(
        &self,
        _: &str,
        _: &Profile,
        _: &str,
    ) -> Result<ProviderTurn, ProviderError> {
        match self.reply {
            Reply::Deferred => return Err(ProviderError::Deferred("busy".into())),
            Reply::Unknown => return Err(ProviderError::Unknown("response lost".into())),
            _ => {}
        }
        self.notifications
            .send(ProviderNotification::TurnStarted {
                thread_id: "provider".into(),
                turn_id: "native-current".into(),
            })
            .unwrap();
        self.release.notified().await;
        match self.reply {
            Reply::Success => Ok(ProviderTurn { turn_id: "native-current".into() }),
            _ => Err(ProviderError::Unknown("late RPC failure".into())),
        }
    }
    async fn steer(&self, _: &str, _: &str, _: &str) -> Result<(), ProviderError> {
        unreachable!("test never steers")
    }
    async fn interrupt(&self, _: &str, _: &str) -> Result<(), ProviderError> {
        unreachable!("test never interrupts")
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_start_authorizes_current_publication_before_start_rpc_response() {
    for old_completed in [false, true] {
        for reply in [Reply::Success, Reply::LateError] {
            let fixture = Fixture::new(true);
            if old_completed {
                fixture
                    .db()
                    .execute("UPDATE turns SET started_at='2099-01-01T00:00:00Z'", [])
                    .unwrap();
            } else {
                fixture.db().execute("DELETE FROM turns", []).unwrap();
            }
            fixture.db().execute_batch("UPDATE provider_sessions SET lifecycle='running'; INSERT INTO turns(turn_id,session_id,context_revision,trigger_kind,lifecycle) VALUES ('current','session','context','wake','starting');").unwrap();
            // An unobserved starting turn must fence publication, not fall back
            // to an older completed turn with a non-null timestamp.
            assert!(
                resolve_source(&fixture.config, Some("issue-codex"), &fixture.invocation())
                    .unwrap_err()
                    .to_string()
                    .contains("publication source was invalidated or superseded")
            );
            let provider = DelayedProvider::new(reply);
            let session = ProviderAgentSession::start(
                provider.clone(),
                fixture.config.profile("issue-codex").unwrap().clone(),
                String::new(),
                None,
            )
            .await
            .unwrap();
            let mut events = session.events();
            let mut witness = session.events();
            let start = crate::group::turn_start::send_and_observe(
                &fixture.store,
                "current",
                "session",
                session.as_ref(),
                &mut events,
                "event references".into(),
            );
            let publish_before_response = async {
                loop {
                    let lifecycle: String = fixture
                        .db()
                        .query_row("SELECT lifecycle FROM turns WHERE turn_id='current'", [], |r| {
                            r.get(0)
                        })
                        .unwrap();
                    if lifecycle == "running" {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                let resolved =
                    resolve_source(&fixture.config, Some("issue-codex"), &fixture.invocation())
                        .unwrap()
                        .unwrap();
                assert_eq!(resolved.turn_id, "current");
                let receipt = fixture.comment(); // same production acceptance path used by the CLI
                assert_eq!(receipt.source.unwrap().turn_id, "current");
                fixture.db().execute("INSERT INTO events(event_id,work_item_node_id,origin,reference,lifecycle,observed_at,kind) VALUES ('invalidate','item','human','changed','pending','now','invalidate')", []).unwrap();
                assert!(
                    resolve_source(&fixture.config, Some("issue-codex"), &fixture.invocation())
                        .unwrap_err()
                        .to_string()
                        .contains("publication source was invalidated or superseded")
                );
                assert!(
                    crate::store::publication_source::check(
                        fixture.config.runtime.database(),
                        &resolved
                    )
                    .unwrap_err()
                    .to_string()
                    .contains("publication source was invalidated or superseded")
                );
                fixture.db().execute("DELETE FROM events WHERE event_id='invalidate'", []).unwrap();
                // Terminal arrives before the delayed response: the dispatch
                // helper must leave it available to its running-turn consumer.
                provider
                    .notifications
                    .send(ProviderNotification::TurnCompleted {
                        thread_id: "provider".into(),
                        turn_id: "native-current".into(),
                        status: "completed".into(),
                        error: None,
                    })
                    .unwrap();
                while !matches!(witness.recv().await.unwrap(), SessionEvent::TurnTerminal { .. }) {}
                provider.release.notify_one();
                resolved
            };
            let (started, resolved) = tokio::time::timeout(Duration::from_secs(5), async {
                tokio::join!(start, publish_before_response)
            })
            .await
            .unwrap();
            assert_eq!(started.unwrap().as_deref(), Some("native-current"));
            assert!(matches!(
                events.recv().await.unwrap(),
                SessionEvent::TurnTerminal { outcome: TurnOutcome::Completed, .. }
            ));
            assert!(matches!(events.try_recv(), Err(broadcast::error::TryRecvError::Empty))); // no duplicated start from the RPC response
            crate::store::publication_source::check(fixture.config.runtime.database(), &resolved)
                .unwrap();
        }
    }
}

#[tokio::test]
async fn unobserved_deferred_and_unknown_start_never_authorize_publication() {
    for reply in [Reply::Deferred, Reply::Unknown] {
        let fixture = Fixture::new(true);
        fixture.db().execute("UPDATE turns SET lifecycle='starting'", []).unwrap();
        let provider = DelayedProvider::new(reply);
        let session = ProviderAgentSession::start(
            provider,
            fixture.config.profile("issue-codex").unwrap().clone(),
            String::new(),
            None,
        )
        .await
        .unwrap();
        let mut events = session.events();
        let result = crate::group::turn_start::send_and_observe(
            &fixture.store,
            "turn",
            "session",
            session.as_ref(),
            &mut events,
            "event references".into(),
        )
        .await;
        match reply {
            Reply::Deferred => assert!(result.unwrap().is_none()),
            Reply::Unknown => assert!(matches!(result, Err(SessionError::Unknown(_)))),
            _ => unreachable!(),
        }
        assert!(
            resolve_source(&fixture.config, None, &Invocation::Agent("turn".into()))
                .unwrap_err()
                .to_string()
                .contains("publication source was invalidated or superseded")
        );
        assert!(matches!(events.try_recv(), Err(broadcast::error::TryRecvError::Empty)));
    }
}
