#[path = "../src/health.rs"]
mod health;

use std::sync::Arc;

use health::{HealthSnapshot, ProviderHealthUpdate, provider_health_worker};
use tokio::sync::{RwLock, mpsc, watch};

async fn aggregate(reports: &[(&str, &'static str, Option<&str>)]) -> HealthSnapshot {
    let snapshot = Arc::new(RwLock::new(HealthSnapshot {
        ready: false,
        ingress: "github".into(),
        repository: "owner/one, owner/two".into(),
        tunnel: "disabled",
        webhook_url: None,
        webhook_sync: "disabled",
        tunnel_verified_at: None,
        tunnel_error: None,
        reconciliation: "starting",
        provider: "starting",
        last_error: None,
    }));
    let (sender, receiver) = mpsc::channel(8);
    for &(repository, group, error) in reports {
        sender
            .send(ProviderHealthUpdate {
                repository: repository.into(),
                group,
                error: error.map(str::to_owned),
            })
            .await
            .unwrap();
    }
    drop(sender);
    let (_stop, shutdown) = watch::channel(false);
    provider_health_worker(Arc::clone(&snapshot), 4, receiver, shutdown).await;
    Arc::try_unwrap(snapshot).unwrap().into_inner()
}

#[tokio::test]
async fn health_tracks_each_repository_and_kind_independently() {
    let mut reports = vec![("owner/one", "issue", None), ("owner/one", "pr", None)];
    assert_eq!(aggregate(&reports).await.provider, "starting");
    reports.extend([("owner/two", "issue", None), ("owner/two", "pr", None)]);
    assert_eq!(aggregate(&reports).await.provider, "connected");

    reports[0].2 = Some("first repository unavailable");
    let failed = aggregate(&reports).await;
    assert_eq!(failed.provider, "unavailable");
    assert_eq!(failed.last_error.as_deref(), Some("first repository unavailable"));

    reports.push(("owner/one", "issue", None));
    let recovered = aggregate(&reports).await;
    assert_eq!(recovered.provider, "connected");
    assert_eq!(recovered.last_error, None);
}
