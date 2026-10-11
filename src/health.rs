//! Operator-facing health snapshot. Leaf module shared by the runtime health
//! server (writer of `ready`/ingress fields, reader for `/health`) and the
//! workers (writers of provider/reconciliation/tunnel fields).

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthSnapshot {
    pub ready: bool,
    pub ingress: String,
    pub repository: String,
    pub tunnel: &'static str,
    pub webhook_url: Option<String>,
    pub reconciliation: &'static str,
    pub provider: &'static str,
    pub last_error: Option<String>,
}

/// Per-driver observations are aggregated before publishing provider health.
pub(crate) struct ProviderHealthUpdate {
    pub(crate) group: &'static str,
    pub(crate) error: Option<String>,
}

pub(crate) async fn provider_health_worker(
    health: std::sync::Arc<tokio::sync::RwLock<HealthSnapshot>>,
    mut reports: tokio::sync::mpsc::Receiver<ProviderHealthUpdate>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut observations = std::collections::BTreeMap::new();
    let mut prior_error = None;
    loop {
        let report = tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            report = reports.recv() => match report { Some(report) => report, None => return },
        };
        observations.insert(report.group, report.error);
        let error = observations.values().find_map(Clone::clone);
        let mut current = health.write().await;
        current.provider = if error.is_some() {
            "unavailable"
        } else if observations.len() == 2 {
            "connected"
        } else {
            "starting"
        };
        if current.ingress == "local" {
            current.ready = current.provider == "connected";
        }
        if error.is_some() || current.last_error == prior_error {
            current.last_error.clone_from(&error);
        }
        prior_error = error;
    }
}
