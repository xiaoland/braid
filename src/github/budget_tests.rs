use super::{GitHubError, graphql, rate_limit};
use axum::{Router, body::Body, http::Response, routing::post};
use octocrab::{Octocrab, service::middleware::retry::RetryConfig};
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn server(
    responses: Vec<Response<Body>>,
) -> (Octocrab, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let count = Arc::new(AtomicUsize::new(0));
    let replies = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::from(responses)));
    let route_count = Arc::clone(&count);
    let app = Router::new().route(
        "/graphql",
        post(move || {
            let count = Arc::clone(&route_count);
            let replies = Arc::clone(&replies);
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                replies.lock().unwrap().pop_front().expect("unexpected API request")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = Octocrab::builder()
        .base_uri(format!("http://{address}"))
        .unwrap()
        .add_retry_config(RetryConfig::None)
        .build()
        .unwrap();
    (client, count, task)
}

fn response(status: u16, headers: &[(&str, String)], body: &str) -> Response<Body> {
    let mut response =
        Response::builder().status(status).header("content-type", "application/json");
    for (name, value) in headers {
        response = response.header(*name, value);
    }
    response.body(Body::from(body.to_owned())).unwrap()
}

#[tokio::test]
async fn graphql_secondary_headers_block_queued_same_installation_but_not_another() {
    let (client, count, task) = server(vec![
        response(429, &[("retry-after", "60".into())], "secondary limit, not JSON"),
        response(200, &[], r#"{"data":{"ok":true}}"#),
    ])
    .await;
    let limits = rate_limit::RateLimits::default();
    let (first, queued) = tokio::join!(
        graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &()),
        graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &()),
    );
    let first = first.unwrap_err();
    assert!(matches!(first, GitHubError::Delayed(_)));
    assert!(matches!(queued, Err(GitHubError::Delayed(_))));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    graphql::request::<_, Value>(&client, &limits, 2, false, "query{viewer{id}}", &())
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn graphql_response_headers_reserve_budget_and_primary_limit() {
    let reset = rate_limit::now() + 3600;
    let (client, count, task) = server(vec![
        response(
            200,
            &[("x-ratelimit-remaining", "100".into()), ("x-ratelimit-reset", reset.to_string())],
            r#"{"data":{"ok":true}}"#,
        ),
        response(
            200,
            &[("x-ratelimit-remaining", "0".into()), ("x-ratelimit-reset", reset.to_string())],
            r#"{"errors":[{"message":"request refused"}]}"#,
        ),
    ])
    .await;
    let limits = rate_limit::RateLimits::default();
    graphql::request::<_, Value>(&client, &limits, 1, true, "query{viewer{id}}", &())
        .await
        .unwrap();
    assert!(matches!(
        graphql::request::<_, Value>(&client, &limits, 1, true, "query{viewer{id}}", &()).await,
        Err(GitHubError::Delayed(_))
    ));
    let first_limit =
        graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &())
            .await
            .unwrap_err();
    assert!(matches!(first_limit, GitHubError::Delayed(_)));
    assert!(matches!(
        graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &()).await,
        Err(GitHubError::Delayed(_))
    ));
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn graphql_and_http_limit_responses_defer_the_installation() {
    let (client, count, task) = server(vec![
        response(200, &[], r#"{"errors":[{"type":"RATE_LIMITED","message":"budget exhausted"}]}"#),
        response(403, &[("retry-after", "60".into())], "secondary limit, not JSON"),
        response(
            403,
            &[],
            r#"{"message":"API rate limit already exceeded for installation ID 3"}"#,
        ),
    ])
    .await;
    let limits = rate_limit::RateLimits::default();
    for installation in [1, 2, 3] {
        let error = graphql::request::<_, Value>(
            &client,
            &limits,
            installation,
            false,
            "query{viewer{id}}",
            &(),
        )
        .await
        .unwrap_err();
        assert!(error.is_unavailable());
        assert!(matches!(error, GitHubError::Delayed(_)));
        assert!(matches!(
            graphql::request::<_, Value>(
                &client,
                &limits,
                installation,
                false,
                "query{viewer{id}}",
                &(),
            )
            .await,
            Err(GitHubError::Delayed(_))
        ));
    }
    assert_eq!(count.load(Ordering::SeqCst), 3);
    task.abort();
}

#[tokio::test]
async fn permission_denial_does_not_cool_down_an_entire_installation() {
    let (client, count, task) = server(vec![
        response(403, &[], r#"{"message":"Resource not accessible by integration"}"#),
        response(200, &[], r#"{"data":{"ok":true}}"#),
    ])
    .await;
    let limits = rate_limit::RateLimits::default();
    let error = graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &())
        .await
        .unwrap_err();
    assert!(!error.is_unavailable());
    assert!(matches!(error, GitHubError::Http { .. }));
    graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &())
        .await
        .unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn graphql_transport_failure_is_deferred_without_repeated_network_requests() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let disconnect = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        drop(stream);
    });
    let client = Octocrab::builder()
        .base_uri(format!("http://{address}"))
        .unwrap()
        .add_retry_config(RetryConfig::None)
        .build()
        .unwrap();
    let limits = rate_limit::RateLimits::default();
    assert!(matches!(
        graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &()).await,
        Err(GitHubError::Octocrab(_))
    ));
    assert!(matches!(
        graphql::request::<_, Value>(&client, &limits, 1, false, "query{viewer{id}}", &()).await,
        Err(GitHubError::Delayed(_))
    ));
    disconnect.await.unwrap();
}
