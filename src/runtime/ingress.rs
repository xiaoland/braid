use super::*;
use crate::runtime::outbox::drain_one_write;
use crate::webhook::{self, WebhookHeaders};

pub(crate) async fn webhook_handler(
    State(state): State<Arc<IngressState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let header = |name: &'static str| headers.get(name).and_then(|value| value.to_str().ok());
    let parsed = webhook::parse_verified(
        WebhookHeaders {
            signature: header("x-hub-signature-256"),
            delivery: header("x-github-delivery"),
            event: header("x-github-event"),
        },
        &body,
        &state.webhook_secret,
        &state.repository,
        &state.handle,
        webhook::ActorPolicy {
            app_node_id: &state.app_actor_node_id,
            app_login: &state.app_actor_login,
            agent_node_ids: &state.agent_actor_node_ids,
            agent_attributions: &state.agent_attributions,
        },
    );
    let event = match parsed {
        Ok(event) => event,
        Err(error) => {
            tracing::warn!(%error, "rejected GitHub webhook");
            let status = if matches!(
                error,
                webhook::WebhookError::MissingSignature | webhook::WebhookError::InvalidSignature
            ) {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::BAD_REQUEST
            };
            return (status, error.to_string()).into_response();
        }
    };
    let span = tracing::info_span!(
        "github.delivery",
        github.delivery = %event.delivery_guid,
        github.event = %event.event_name,
        github.action = event.action.as_deref().unwrap_or("")
    );
    let _entered = span.enter();
    telemetry::emit_payload_event(&PayloadEvidence {
        github_body: "",
        github_summary: &event.reference,
        credential: "",
        provider_transcript: "",
        webhook_payload: std::str::from_utf8(&body).unwrap_or("<non-UTF-8 webhook>"),
        local_path: "",
    });
    match state.store.ingest_event(event, state.policy) {
        Ok(result) => {
            let status = if result.duplicate { StatusCode::OK } else { StatusCode::ACCEPTED };
            (status, axum::Json(result)).into_response()
        }
        Err(error) => {
            tracing::error!(%error, "cannot durably ingest GitHub webhook");
            (StatusCode::SERVICE_UNAVAILABLE, "durable ingest unavailable").into_response()
        }
    }
}

pub(crate) async fn event_worker(
    store: Arc<StoreActor>,
    github: Arc<GitHubClient>,
    policy: SchedulerPolicy,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tick.tick() => {
                if let Err(error) = store.advance_scheduler() {
                    tracing::error!(%error, "cannot advance scheduler");
                }
                match store.mention_candidates(16) {
                    Ok(candidates) => {
                        for candidate in candidates {
                            match github.repository_permission(&candidate.actor_login).await {
                                Ok(role) => {
                                    let trusted = matches!(role.to_ascii_lowercase().as_str(), "maintain" | "admin");
                                    if let Err(error) = store.resolve_mention(candidate.event_id, trusted, policy) {
                                        tracing::error!(%error, "cannot resolve mention authority");
                                    }
                                }
                                Err(error) => tracing::warn!(%error, actor = %candidate.actor_login, "mention authority remains unresolved"),
                            }
                        }
                    }
                    Err(error) => tracing::error!(%error, "cannot load mention candidates"),
                }
                drain_one_write(&store, &github).await;
            }
        }
    }
}
