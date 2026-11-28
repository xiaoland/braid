use super::*;
use crate::group::{
    GroupKind::{self, Issue, Pr},
    availability::{ProviderReport, track_provider_availability},
};
use tokio::{sync::mpsc, time::Duration};

fn health(ingress: &str) -> Health {
    Health::new(HealthSnapshot {
        ready: false,
        ingress: ingress.into(),
        repository: "owner/one, owner/two".into(),
        tunnel: "disabled",
        webhook_url: None,
        webhook_sync: "disabled",
        tunnel_verified_at: None,
        tunnel_error: None,
        reconciliation: "starting",
        provider: "starting",
        last_error: None,
    })
}

async fn aggregate(reports: &[(&str, GroupKind, Option<&str>)]) -> HealthSnapshot {
    let health = health("github");
    let (sender, receiver) = mpsc::channel(8);
    for &(repository, group, error) in reports {
        sender
            .send(ProviderReport {
                repository: repository.into(),
                group,
                error: error.map(str::to_owned),
            })
            .await
            .unwrap();
    }
    drop(sender);
    let (_stop, shutdown) = watch::channel(false);
    let (availability, provider_status) = watch::channel(ProviderAvailability::Starting);
    tokio::join!(
        track_provider_availability(4, receiver, availability, shutdown.clone()),
        provider_health_worker(health.clone(), provider_status, shutdown),
    );
    health.snapshot()
}

#[tokio::test]
async fn health_tracks_each_repository_and_kind_independently() {
    let mut reports = vec![("owner/one", Issue, None), ("owner/one", Pr, None)];
    assert_eq!(aggregate(&reports).await.provider, "starting");
    reports.extend([("owner/two", Issue, None), ("owner/two", Pr, None)]);
    assert_eq!(aggregate(&reports).await.provider, "connected");

    reports[0].2 = Some("first repository unavailable");
    let failed = aggregate(&reports).await;
    assert_eq!(failed.provider, "unavailable");
    assert_eq!(failed.last_error.as_deref(), Some("first repository unavailable"));

    reports.push(("owner/one", Issue, None));
    let recovered = aggregate(&reports).await;
    assert_eq!(recovered.provider, "connected");
    assert_eq!(recovered.last_error, None);
}

#[tokio::test]
async fn provider_control_advances_while_health_projection_is_paused() {
    let health = health("builtin");
    let earlier_snapshot = health.snapshot();
    let (_stop, shutdown) = watch::channel(false);
    let (reports, receiver) = mpsc::channel(1);
    let (availability, mut control) = watch::channel(ProviderAvailability::Starting);
    let tracker =
        tokio::spawn(track_provider_availability(1, receiver, availability, shutdown.clone()));
    let projection = provider_health_worker(health.clone(), control.clone(), shutdown);

    let send = |error: Option<&str>| {
        reports.send(ProviderReport {
            repository: "owner/one".into(),
            group: Issue,
            error: error.map(str::to_owned),
        })
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        send(Some("first failure")).await.unwrap();
        control.changed().await.unwrap();
        let initial_failure = match &*control.borrow() {
            ProviderAvailability::Unavailable { since, error } => {
                assert_eq!(error, "first failure");
                *since
            }
            status => panic!("expected unavailable, got {status:?}"),
        };
        for _ in 0..3 {
            send(Some("retry failure")).await.unwrap();
            control.changed().await.unwrap();
            match &*control.borrow() {
                ProviderAvailability::Unavailable { since, error } => {
                    assert_eq!(*since, initial_failure);
                    assert_eq!(error, "retry failure");
                }
                status => panic!("expected unavailable during recovery, got {status:?}"),
            }
        }
        send(None).await.unwrap();
        control.changed().await.unwrap();
        assert!(matches!(*control.borrow(), ProviderAvailability::Connected));
    })
    .await
    .unwrap();

    assert_eq!(health.snapshot().provider, "starting");
    drop(reports);
    tracker.await.unwrap();
    projection.await;
    let latest = health.snapshot();
    assert_eq!(latest.provider, "connected");
    assert!(latest.ready);
    assert_eq!(latest.last_error, None);
    assert_eq!(earlier_snapshot.provider, "starting");
    assert!(!earlier_snapshot.ready);
}
