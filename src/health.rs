//! Operator-facing health snapshot. Shared by the runtime health
//! server (writer of `ready`/ingress fields, reader for `/health`) and the
//! workers (writers of provider/reconciliation/tunnel fields).

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::watch;

use crate::group::availability::ProviderAvailability;

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthSnapshot {
    pub ready: bool,
    pub ingress: String,
    pub repository: String,
    pub tunnel: &'static str,
    pub webhook_url: Option<String>,
    pub webhook_sync: &'static str,
    pub tunnel_verified_at: Option<String>,
    pub tunnel_error: Option<String>,
    pub reconciliation: &'static str,
    pub provider: &'static str,
    pub last_error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Health(Arc<Mutex<HealthSnapshot>>);

impl Health {
    pub(crate) fn new(initial: HealthSnapshot) -> Self {
        Self(Arc::new(Mutex::new(initial)))
    }

    /// Return an owned copy for formatting and serialization.
    pub(crate) fn snapshot(&self) -> HealthSnapshot {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Update fields in one short, synchronous critical section without I/O.
    pub(crate) fn update<T>(&self, apply: impl FnOnce(&mut HealthSnapshot) -> T) -> T {
        apply(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

pub(crate) async fn provider_health_worker(
    health: Health,
    mut availability: watch::Receiver<ProviderAvailability>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut prior_error = None;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return,
            changed = availability.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
        let (provider, error) = match &*availability.borrow_and_update() {
            ProviderAvailability::Starting => ("starting", None),
            ProviderAvailability::Connected => ("connected", None),
            ProviderAvailability::Unavailable { error, .. } => ("unavailable", Some(error.clone())),
        };
        health.update(|current| {
            current.provider = provider;
            if current.ingress == "builtin" {
                current.ready = provider == "connected";
            }
            if error.is_some() || current.last_error == prior_error {
                current.last_error.clone_from(&error);
            }
        });
        prior_error = error;
    }
}

#[cfg(test)]
mod tests;
