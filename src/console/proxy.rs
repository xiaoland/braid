//! The browser only reaches the platform endpoint declared by the operator.
use anyhow::{Context as _, Result, ensure};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use std::time::Duration;
use url::Url;

#[derive(Clone)]
struct PlatformProxy {
    endpoint: Url,
    client: reqwest::Client,
}

pub fn router(endpoint: Url) -> Result<Router> {
    ensure!(
        endpoint.scheme() == "http"
            && match endpoint.host() {
                Some(url::Host::Domain(host)) => host == "localhost",
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                None => false,
            }
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.query().is_none()
            && endpoint.fragment().is_none()
            && endpoint.path() == "/",
        "Console platform endpoint must be a credential-free loopback HTTP origin"
    );
    let state = PlatformProxy {
        endpoint,
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("create platform proxy client")?,
    };
    Ok(Router::new()
        .route("/api/control/snapshot", get(forward))
        .route("/api/control/items/{number}", get(forward))
        .route("/api/control/items/{number}/diff", get(forward))
        .route("/api/control/commands", post(forward))
        .route("/api/control/receipts/{request_id}", get(forward))
        .with_state(state))
}

async fn forward(State(state): State<PlatformProxy>, request: Request) -> Response {
    match forward_request(state, request).await {
        Ok(response) => response,
        Err(error) => (
            StatusCode::BAD_GATEWAY,
            axum::Json(serde_json::json!({"error":format!("platform request failed: {error:#}")})),
        )
            .into_response(),
    }
}

async fn forward_request(state: PlatformProxy, request: Request) -> Result<Response> {
    let (parts, body) = request.into_parts();
    let url = state.endpoint.join(parts.uri.path()).context("resolve platform API route")?;
    ensure!(
        url.origin() == state.endpoint.origin() && url.path() == parts.uri.path(),
        "platform API path must not normalize outside the declared route"
    );
    let mut builder = state.client.request(parts.method.clone(), url);
    if parts.method != Method::GET {
        let body = to_bytes(body, super::collector::MAX_BATCH_BYTES)
            .await
            .context("read platform command body")?;
        builder = builder
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ORIGIN, state.endpoint.origin().ascii_serialization())
            .body(body);
    }
    let upstream = builder.send().await.context("send declared platform API request")?;
    let status = upstream.status();
    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let body = upstream.bytes().await.context("read platform API response body")?;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    if let Some(content_type) = content_type {
        response.headers_mut().insert(header::CONTENT_TYPE, content_type);
    }
    Ok(response)
}
