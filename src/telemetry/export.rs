//! Observe SDK exporter calls, not backend persistence or retry/queue internals.
use std::sync::{Arc, Mutex};
use std::time::Duration;

use opentelemetry_sdk::{
    Resource,
    error::{OTelSdkError, OTelSdkResult},
    logs::{LogBatch, LogExporter},
    metrics::{Temporality, data::ResourceMetrics, exporter::PushMetricExporter},
    trace::{SpanData, SpanExporter},
};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ExportObservation {
    pub endpoint: String,
    pub signal: &'static str,
    pub stage: &'static str,
    pub successful_batches: u64,
    pub failed_batches: u64,
    /// Records in failed SDK calls, not an assertion about backend receipt.
    pub failed_records: u64,
    pub queue_dropped_records: Option<u64>,
    pub last_failure: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct ExportState(Arc<Mutex<ExportObservation>>);

impl ExportState {
    pub fn new(endpoint: &str, signal: &'static str) -> Self {
        // Endpoint diagnostics must not expose URL credentials or query tokens.
        let endpoint = url::Url::parse(endpoint).map_or_else(
            |_| "<invalid endpoint>".to_owned(),
            |mut url| {
                let _ = url.set_username("");
                let _ = url.set_password(None);
                url.set_query(None);
                url.set_fragment(None);
                url.to_string()
            },
        );
        Self(Arc::new(Mutex::new(ExportObservation {
            endpoint,
            signal,
            stage: "sdk_export",
            successful_batches: 0,
            failed_batches: 0,
            failed_records: 0,
            queue_dropped_records: None,
            last_failure: None,
        })))
    }

    pub fn snapshot(&self) -> ExportObservation {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    fn complete(&self, records: usize, result: &OTelSdkResult) {
        let mut state = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if result.is_ok() {
            state.successful_batches += 1;
        } else {
            let category = match result.as_ref().expect_err("failed result") {
                OTelSdkError::AlreadyShutdown => "already_shutdown",
                OTelSdkError::Timeout(_) => "sdk_timeout",
                OTelSdkError::InternalFailure(_) => "sdk_internal_failure",
            };
            state.last_failure = Some(category);
            state.failed_batches += 1;
            state.failed_records += records as u64;
            // Do not trace exporter diagnostics back into the failed exporter.
            // SDK errors can contain remote response bodies: never print those.
            eprintln!(
                "OTLP endpoint={} signal={} stage=sdk_export outcome=failed failed_batches={} failed_records={} queue_dropped_records=unknown error_category={} detail=unavailable_at_sdk_boundary",
                state.endpoint, state.signal, state.failed_batches, state.failed_records, category,
            );
        }
    }
}

#[derive(Debug)]
pub struct ObservedExporter<E> {
    inner: E,
    state: ExportState,
}

impl<E> ObservedExporter<E> {
    pub fn new(inner: E, state: ExportState) -> Self {
        Self { inner, state }
    }
}

impl<E: SpanExporter> SpanExporter for ObservedExporter<E> {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let count = batch.len();
        let result = self.inner.export(batch).await;
        self.state.complete(count, &result);
        result
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

impl<E: LogExporter> LogExporter for ObservedExporter<E> {
    async fn export(&self, batch: LogBatch<'_>) -> OTelSdkResult {
        let count = batch.iter().count();
        let result = self.inner.export(batch).await;
        self.state.complete(count, &result);
        result
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
    fn event_enabled(
        &self,
        level: opentelemetry::logs::Severity,
        target: &str,
        name: Option<&str>,
    ) -> bool {
        self.inner.event_enabled(level, target, name)
    }
}

impl<E: PushMetricExporter> PushMetricExporter for ObservedExporter<E> {
    async fn export(&self, metrics: &ResourceMetrics) -> OTelSdkResult {
        let result = self.inner.export(metrics).await;
        // One ResourceMetrics envelope; do not mislabel it as a datapoint count.
        self.state.complete(1, &result);
        result
    }
    fn force_flush(&self) -> OTelSdkResult {
        self.inner.force_flush()
    }
    fn shutdown_with_timeout(&self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }
    fn temporality(&self) -> Temporality {
        self.inner.temporality()
    }
}
