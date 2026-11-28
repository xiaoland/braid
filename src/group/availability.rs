//! Aggregate provider recovery reports from the Agent Groups.

use std::collections::BTreeMap;

use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

use super::GroupKind;

#[derive(Debug)]
pub(crate) enum ProviderAvailability {
    Starting,
    Connected,
    Unavailable { since: Instant, error: String },
}

pub(crate) struct ProviderReport {
    pub(crate) repository: String,
    pub(crate) group: GroupKind,
    pub(crate) error: Option<String>,
}

pub(crate) async fn track_provider_availability(
    expected_groups: usize,
    mut reports: mpsc::Receiver<ProviderReport>,
    availability: watch::Sender<ProviderAvailability>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut observations = BTreeMap::new();
    let mut unavailable_since = None;
    loop {
        let report = tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            report = reports.recv() => match report { Some(report) => report, None => return },
        };
        observations.insert((report.repository, report.group), report.error);
        let next = if let Some(error) = observations.values().find_map(Clone::clone) {
            let since = *unavailable_since.get_or_insert_with(Instant::now);
            ProviderAvailability::Unavailable { since, error }
        } else {
            unavailable_since = None;
            if observations.len() == expected_groups {
                ProviderAvailability::Connected
            } else {
                ProviderAvailability::Starting
            }
        };
        availability.send_replace(next);
    }
}
