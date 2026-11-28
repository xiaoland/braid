use super::*;
use crate::group::{
    GroupKind::{self, Issue, Pr},
    availability::{ProviderReport, track_provider_availability},
};
use tokio::{sync::mpsc, time::Duration};

fn health() -> Health {
    Health::new(HealthSnapshot {
        ready: false,
        ingress: "builtin".into(),
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

#[tokio::test]
async fn provider_availability_tracks_each_group_and_preserves_the_outage_start() {
    use ProviderAvailability::{Connected, Starting, Unavailable};

    let (_stop, shutdown) = watch::channel(false);
    let (reports, receiver) = mpsc::channel(8);
    let (availability, mut status) = watch::channel(Starting);
    let tracker = tokio::spawn(track_provider_availability(4, receiver, availability, shutdown));
    let send = |repository: &str, group: GroupKind, error: Option<&str>| {
        reports
            .try_send(ProviderReport {
                repository: repository.into(),
                group,
                error: error.map(str::to_owned),
            })
            .unwrap();
    };
    tokio::time::timeout(Duration::from_secs(2), async {
        send("owner/one", Issue, Some("offline"));
        let Unavailable { since: first_failure, .. } =
            *status.wait_for(|s| matches!(s, Unavailable { .. })).await.unwrap()
        else {
            unreachable!()
        };
        send("owner/one", Pr, None);
        send("owner/one", Issue, Some("retry"));
        let retry = status
            .wait_for(|s| matches!(s, Unavailable { error, .. } if error == "retry"))
            .await
            .unwrap();
        assert!(matches!(*retry, Unavailable { since, .. } if since == first_failure));
        drop(retry);

        send("owner/one", Issue, None);
        status.wait_for(|s| matches!(s, Starting)).await.unwrap();
        send("owner/two", Issue, None);
        send("owner/two", Pr, None);
        status.wait_for(|s| matches!(s, Connected)).await.unwrap();
    })
    .await
    .expect("provider availability must advance without an operator health consumer");
    send("owner/one", Issue, Some("still offline"));
    send("owner/two", Pr, None);
    drop(reports);
    tracker.await.unwrap();
    assert!(matches!(&*status.borrow(), Unavailable { error, .. } if error == "still offline"));
}

#[tokio::test]
async fn provider_recovery_clears_its_own_diagnostic_and_preserves_another_workers_error() {
    for other_error in [None, Some("reconciliation failed")] {
        let health = health();
        let (_stop, shutdown) = watch::channel(false);
        let (availability, reports) = watch::channel(ProviderAvailability::Starting);
        let worker = tokio::spawn(provider_health_worker(health.clone(), reports, shutdown));
        availability.send_replace(ProviderAvailability::Unavailable {
            since: tokio::time::Instant::now(),
            error: "provider failed".into(),
        });
        // The projection must have published the failure before the recovery
        // report: a watch receiver can legally coalesce adjacent updates.
        tokio::time::timeout(Duration::from_secs(2), async {
            while health.snapshot().provider != "unavailable" {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("health must publish the initial provider failure");
        assert_eq!(health.snapshot().last_error.as_deref(), Some("provider failed"));
        if let Some(other) = other_error {
            health.update(|snapshot| snapshot.last_error = Some(other.into()));
        }
        availability.send_replace(ProviderAvailability::Connected);
        drop(availability);
        worker.await.unwrap();
        let recovered = health.snapshot();
        assert_eq!(recovered.provider, "connected");
        assert!(recovered.ready);
        assert_eq!(recovered.last_error.as_deref(), other_error);
    }
}
