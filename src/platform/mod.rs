pub mod api;
pub mod cli;
pub mod config;
pub mod grants;
pub mod objects;
pub mod store;
pub mod writes;
pub use api::router;
pub use config::PlatformConfig;
pub async fn serve(
    mut config: PlatformConfig,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let listener = tokio::net::TcpListener::bind(&config.listen).await?;
    config.listen = listener.local_addr()?.to_string();
    let router = router(&config)?;
    tracing::info!(listen=%config.listen,"platform listener ready");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            while !*stop.borrow() {
                if stop.changed().await.is_err() {
                    break;
                }
            }
        })
        .await?;
    Ok(())
}
