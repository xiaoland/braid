//! Observe native start identity while the start RPC is still in flight.
use crate::{
    agent_session::{AgentSession, SendResult, SessionError, SessionEvent},
    store::StoreActor,
};
use tokio::sync::broadcast;

pub(crate) async fn send_and_observe(
    store: &StoreActor,
    turn_id: &str,
    session_id: &str,
    session: &dyn AgentSession,
    events: &mut broadcast::Receiver<SessionEvent>,
    reference: String,
) -> Result<Option<String>, SessionError> {
    let send = session.send_user_msg(reference, false);
    tokio::pin!(send);
    let mut observed = None;
    let result = tokio::select! {
        result = &mut send => result,
        event = events.recv() => {
            observed = Some(record_start(store, turn_id, session_id, event));
            // Do not cancel the RPC after native lifecycle wins the race. In
            // particular, the adapter must reconcile a late response/error.
            send.await
        }
    };
    if let Some(observed) = observed {
        // Native lifecycle remains authoritative. Leave terminal events on the
        // same receiver for the running-turn drive loop.
        return observed.map(Some);
    }
    match result? {
        SendResult::Started => {
            record_start(store, turn_id, session_id, events.recv().await).map(Some)
        }
        SendResult::Deferred => Ok(None),
        SendResult::Acknowledged => Err(SessionError::Failed(
            "AgentSession acknowledged a start request without creating a turn".into(),
        )),
    }
}

fn record_start(
    store: &StoreActor,
    turn_id: &str,
    session_id: &str,
    event: Result<SessionEvent, broadcast::error::RecvError>,
) -> Result<String, SessionError> {
    let provider_turn_id = match event {
        Ok(SessionEvent::TurnStarted { provider_turn_id }) => provider_turn_id,
        other => {
            return Err(SessionError::Unknown(format!(
                "AgentSession did not expose the accepted start identity: {other:?}"
            )));
        }
    };
    crate::telemetry::bind_provider_turn(session_id, &provider_turn_id);
    store.mark_turn_started(turn_id.into(), provider_turn_id.clone()).map_err(|error| {
        SessionError::Unknown(format!(
            "accepted provider turn {provider_turn_id} could not be recorded: {error}"
        ))
    })?;
    Ok(provider_turn_id)
}
