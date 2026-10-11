mod api;
mod collector;
mod runtime;
mod storage;

use crate::{config::Config, store::StoreActor, telemetry::TelemetryGuard};
use anyhow::{Context as _, Result, bail};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Request},
    http::{Method, StatusCode, header},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::get,
};
use clap::Args;
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};

#[derive(Debug, Args)]
pub struct ConsoleArgs {
    /// Loopback address for OTLP ingestion and the local Web Console.
    #[arg(long, default_value = "127.0.0.1:4318")]
    pub listen: SocketAddr,
    /// Absolute path of the Console telemetry database, separate from runtime state.
    #[arg(long)]
    pub database: PathBuf,
    /// Local collaboration configuration; enables direct object and runtime control.
    #[arg(long)]
    pub config: Option<PathBuf>,
}

pub async fn serve(args: ConsoleArgs) -> Result<()> {
    if !args.listen.ip().is_loopback() {
        bail!("Console must listen on a loopback address");
    }
    if !args.database.is_absolute() {
        bail!("Console database path must be absolute");
    }
    let listener =
        tokio::net::TcpListener::bind(args.listen).await.context("bind Console listener")?;
    let address = listener.local_addr()?;
    let mut config = args
        .config
        .map(|path| {
            if !path.is_absolute() {
                bail!("Console config path must be absolute");
            }
            Config::load(&path).map_err(anyhow::Error::new)
        })
        .transpose()?;
    if let Some(config) = &config {
        if !config.is_local() {
            bail!(
                "managed Console requires a local collaboration configuration; GitHub OTLP can still be collected without --config"
            );
        }
        if resolved_path(config.runtime.database())? == resolved_path(&args.database)? {
            bail!("Console telemetry and runtime databases must be separate");
        }
    }
    let mut manager = None;
    let mut telemetry = None;
    let storage = Arc::new(Mutex::new(storage::Storage::open(&args.database)?));
    let mut app = api::routes(api::ApiState { storage });
    if let Some(mut config) = config.take() {
        let store = StoreActor::start(
            config.runtime.database().to_path_buf(),
            config.runtime.backups().to_path_buf(),
        )?;
        if !store.plan()?.pending.is_empty() {
            if !config.runtime.auto_migrate {
                bail!(
                    "database needs migration before Console control; use braid migrate apply --config <PATH>"
                );
            }
            store.apply()?;
        }
        // This process owns the collector, so its managed runtime exports to this listener.
        config.telemetry.endpoint = url::Url::parse(&format!("http://{address}"))?;
        telemetry = Some(TelemetryGuard::install(&config.telemetry, config.instance_key())?);
        app = app.merge(crate::control::router(&config)?);
        let runtime = runtime::Manager::new(config)?;
        app = app.merge(runtime.router());
        manager = Some(runtime);
    }
    app = app
        .merge(web_routes())
        .layer(DefaultBodyLimit::max(collector::MAX_BATCH_BYTES))
        .layer(middleware::from_fn(move |request, next| same_origin(request, next, address)));
    println!("Braid Console http://{address}");
    let stop_manager = manager.clone();
    let flush_result = Arc::new(Mutex::new(None));
    let shutdown_flush = Arc::clone(&flush_result);
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = crate::runtime::wait_for_shutdown_signal().await;
            if let Some(manager) = stop_manager
                && let Err(error) = manager.shutdown().await
            {
                tracing::error!(%error,"Console could not confirm runtime stop");
            }
            // Flush while the local Collector can still accept the final runtime evidence.
            if let Some(telemetry) = telemetry {
                let result = tokio::task::spawn_blocking(move || telemetry.shutdown())
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result.map_err(|error| error.to_string()));
                if let Ok(mut saved) = shutdown_flush.lock() {
                    *saved = Some(result);
                }
            }
        })
        .await
        .context("serve Console");
    if let Some(manager) = manager {
        manager.shutdown().await?;
    }
    if let Some(Err(error)) = flush_result
        .lock()
        .map_err(|_| anyhow::anyhow!("telemetry shutdown result lock poisoned"))?
        .take()
    {
        bail!("Console telemetry shutdown failed: {error}");
    }
    result
}

fn web_routes() -> Router {
    Router::new()
        .route("/", get(|| async { Html(include_str!("web/index.html")) }))
        .route(
            "/console.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("web/console.css"),
                )
            }),
        )
        .route(
            "/console.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("web/console.js"),
                )
            }),
        )
}
async fn same_origin(request: Request, next: Next, address: SocketAddr) -> Response {
    let host =
        request.headers().get(header::HOST).and_then(|value| value.to_str().ok()).unwrap_or("");
    let allowed = host == address.to_string() || host == format!("localhost:{}", address.port());
    if !allowed {
        return rejected_request(&request, "Console Host must match its loopback listener");
    }
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && let Some(origin) = request.headers().get(header::ORIGIN)
        && origin.to_str().ok() != Some(format!("http://{host}").as_str())
    {
        return rejected_request(&request, "Console mutations require a same-origin request");
    }
    let mut response = next.run(request).await;
    response.headers_mut().insert(header::CONTENT_SECURITY_POLICY,axum::http::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'"));
    response
        .headers_mut()
        .insert(header::X_CONTENT_TYPE_OPTIONS, axum::http::HeaderValue::from_static("nosniff"));
    response
}

fn resolved_path(path: &std::path::Path) -> Result<PathBuf> {
    if path.exists() {
        return path.canonicalize().context("resolve database path");
    }
    let parent = path.parent().context("database path has no parent")?;
    let name = path.file_name().context("database path has no filename")?;
    Ok(resolved_path(parent)?.join(name))
}

fn rejected_request(request: &Request, message: &str) -> Response {
    if matches!(request.uri().path(), "/v1/traces" | "/v1/logs" | "/v1/metrics") {
        api::otlp_failure(StatusCode::FORBIDDEN, message.into())
    } else {
        (StatusCode::FORBIDDEN, message.to_owned()).into_response()
    }
}
