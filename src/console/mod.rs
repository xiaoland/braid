mod api;
mod collector;
mod harness;
mod proxy;
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
    /// Optional Harness configuration for either GitHub or the built-in platform.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Independent built-in platform configuration; manages its listener for the Console lifetime.
    #[arg(long)]
    pub platform_config: Option<PathBuf>,
}

// Keep startup and reverse-order shutdown together so each managed lifetime is visible.
#[allow(clippy::too_many_lines)]
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
    let mut platform = args
        .platform_config
        .map(|path| {
            if !path.is_absolute() {
                bail!("Console platform config path must be absolute");
            }
            crate::platform::PlatformConfig::load(&path)
        })
        .transpose()?;
    let mut databases = vec![resolved_path(&args.database)?];
    if let Some(config) = &config {
        databases.push(resolved_path(config.runtime.database())?);
    }
    if let Some(platform) = &platform {
        databases.push(resolved_path(&platform.database)?);
    }
    for (i, path) in databases.iter().enumerate() {
        if databases[..i].contains(path) {
            bail!("Console, Harness and platform databases must be separate");
        }
    }
    let mut platform_listener = None;
    let mut platform_endpoint =
        config.as_ref().and_then(|c| c.builtin.as_ref().map(|b| b.endpoint.clone()));
    if let Some(platform) = &mut platform {
        let listener = tokio::net::TcpListener::bind(&platform.listen)
            .await
            .context("bind platform listener")?;
        platform.listen = listener.local_addr()?.to_string();
        let endpoint = url::Url::parse(&format!("http://{}", platform.listen))?;
        if let Some(builtin) = config.as_ref().and_then(|c| c.builtin.as_ref())
            && (builtin.repository != platform.repository || builtin.endpoint != endpoint)
        {
            bail!(
                "managed platform repository and listener must match the Harness builtin declaration"
            );
        }
        platform_endpoint = Some(endpoint);
        platform_listener = Some(listener);
    }
    let metadata = serde_json::json!({
        "harness":{"configured":config.is_some(),"backend":config.as_ref().map(|c|if c.is_builtin(){"builtin"}else{"github"})},
        "platform":{"configured":platform_endpoint.is_some(),"endpoint":platform_endpoint,"managed":platform.is_some()},
        "collector":{"endpoint":format!("http://{address}")}
    });
    let mut manager = None;
    let mut telemetry = None;
    let storage = Arc::new(Mutex::new(storage::Storage::open(&args.database)?));
    let mut app = api::routes(api::ApiState { storage });
    if let Some(endpoint) = platform_endpoint {
        app = app.merge(proxy::router(endpoint)?);
    }
    app = app.route(
        "/api/console",
        get(move || {
            let metadata = metadata.clone();
            async move { axum::Json(metadata) }
        }),
    );
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
        app = app.merge(harness::router(&config));
        let runtime = runtime::Manager::new(config)?;
        app = app.merge(runtime.router());
        manager = Some(runtime);
    }
    if telemetry.is_none()
        && let Some(platform) = &platform
    {
        let platform_telemetry = crate::config::TelemetryConfig {
            endpoint: url::Url::parse(&format!("http://{address}"))?,
            sample_ratio: 0.1,
            incident_mode: false,
            export_timeout_seconds: 5,
            service_name: "braid".into(),
            log_format: crate::config::LogFormat::Text,
        };
        telemetry = Some(TelemetryGuard::install(
            &platform_telemetry,
            &format!("braid-console-platform:{}", platform.repository),
        )?);
    }
    app = app
        .merge(web_routes())
        .layer(DefaultBodyLimit::max(collector::MAX_BATCH_BYTES))
        .layer(middleware::from_fn(move |request, next| same_origin(request, next, address)));
    let (platform_stop, platform_task) =
        if let (Some(platform), Some(listener)) = (platform, platform_listener) {
            let router = crate::platform::router(&platform)?;
            let (stop, mut receiver) = tokio::sync::watch::channel(false);
            let task = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move {
                        while !*receiver.borrow() {
                            if receiver.changed().await.is_err() {
                                break;
                            }
                        }
                    })
                    .await
                    .context("serve managed platform")
            });
            (Some(stop), Some(task))
        } else {
            (None, None)
        };
    println!("Braid Console http://{address}");
    let stop_manager = manager.clone();
    let shutdown_errors = Arc::new(Mutex::new(Vec::new()));
    let saved_errors = shutdown_errors.clone();
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let mut errors = Vec::new();
            if let Err(error) = crate::runtime::wait_for_shutdown_signal().await {
                errors.push(format!("shutdown signal: {error:#}"));
            }
            if let Some(manager) = stop_manager
                && let Err(error) = manager.shutdown().await
            {
                errors.push(format!("Harness stop unconfirmed: {error:#}"));
            }
            if let Some(stop) = platform_stop {
                let _ = stop.send(true);
            }
            if let Some(task) = platform_task {
                match task.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => errors.push(format!("platform shutdown: {error:#}")),
                    Err(error) => errors.push(format!("platform task: {error}")),
                }
            }
            // The Collector still accepts final evidence while the SDK flushes.
            if let Some(telemetry) = telemetry {
                match tokio::task::spawn_blocking(move || telemetry.shutdown()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => errors.push(format!("telemetry shutdown: {error:#}")),
                    Err(error) => errors.push(format!("telemetry shutdown task: {error}")),
                }
            }
            if let Ok(mut saved) = saved_errors.lock() {
                *saved = errors;
            }
        })
        .await
        .context("serve Console");
    if let Some(manager) = manager {
        manager.shutdown().await?;
    }
    let errors = shutdown_errors
        .lock()
        .map_err(|_| anyhow::anyhow!("Console shutdown result lock poisoned"))?;
    if !errors.is_empty() {
        bail!("Console shutdown failed: {}", errors.join("; "));
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
