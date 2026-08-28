use std::time::Duration;

use opentelemetry::{KeyValue, metrics::MeterProvider as _, trace::TraceContextExt as _};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_otlp::{Protocol, WithExportConfig as _};
use opentelemetry_sdk::{
    Resource,
    logs::SdkLoggerProvider,
    metrics::SdkMeterProvider,
    trace::{Sampler, SdkTracerProvider},
};
use serde::Serialize;
use thiserror::Error;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;
use tracing_subscriber::{
    EnvFilter, Layer as _, layer::SubscriberExt as _, util::SubscriberInitExt as _,
};

use crate::config::{LogFormat, TelemetryConfig};

#[derive(Debug, Error)]
pub enum TelemetryError {
    #[error("cannot construct OTLP exporter: {0}")]
    Exporter(String),
    #[error("cannot install telemetry subscriber: {0}")]
    Subscriber(String),
    #[error("cannot flush {signal} telemetry: {message}")]
    Flush { signal: &'static str, message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ExporterHealth {
    pub endpoint: String,
    pub protocol: &'static str,
    pub configured_sample_ratio: f64,
    pub effective_sample_ratio: f64,
    pub incident_mode: bool,
    pub initialized: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub sampled: bool,
    pub payload_emitted: bool,
    pub exporter: ExporterHealth,
}

#[derive(Debug, Clone)]
pub struct PayloadEvidence<'a> {
    pub github_body: &'a str,
    pub github_summary: &'a str,
    pub credential: &'a str,
    pub provider_transcript: &'a str,
    pub webhook_payload: &'a str,
    pub local_path: &'a str,
}

pub struct TelemetryGuard {
    tracer_provider: SdkTracerProvider,
    meter_provider: SdkMeterProvider,
    logger_provider: SdkLoggerProvider,
    health: ExporterHealth,
}

impl TelemetryGuard {
    pub fn install(config: &TelemetryConfig, instance_key: &str) -> Result<Self, TelemetryError> {
        let endpoint = config.endpoint.as_str().trim_end_matches('/').to_owned();
        let trace_endpoint = format!("{endpoint}/v1/traces");
        let metric_endpoint = format!("{endpoint}/v1/metrics");
        let log_endpoint = format!("{endpoint}/v1/logs");
        let timeout = Duration::from_secs(config.export_timeout_seconds);
        let resource = Resource::builder()
            .with_service_name(config.service_name.clone())
            .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
            .with_attribute(KeyValue::new("service.instance.id", instance_key.to_owned()))
            .build();

        let span_exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(trace_endpoint)
            .with_timeout(timeout)
            .build()
            .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
        let tracer_provider = SdkTracerProvider::builder()
            .with_resource(resource.clone())
            .with_sampler(Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(
                config.effective_sample_ratio(),
            ))))
            .with_batch_exporter(span_exporter)
            .build();

        let metric_exporter = opentelemetry_otlp::MetricExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(metric_endpoint)
            .with_timeout(timeout)
            .build()
            .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
        let meter_provider = SdkMeterProvider::builder()
            .with_resource(resource.clone())
            .with_periodic_exporter(metric_exporter)
            .build();

        let log_exporter = opentelemetry_otlp::LogExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_endpoint(log_endpoint)
            .with_timeout(timeout)
            .build()
            .map_err(|error| TelemetryError::Exporter(error.to_string()))?;
        let logger_provider = SdkLoggerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(log_exporter)
            .build();

        let tracer = opentelemetry::trace::TracerProvider::tracer(&tracer_provider, "braid");
        let trace_layer = tracing_opentelemetry::layer().with_tracer(tracer);
        let log_filter = EnvFilter::new("info")
            .add_directive("opentelemetry=off".parse().expect("valid filter"))
            .add_directive("reqwest=off".parse().expect("valid filter"));
        let log_layer = OpenTelemetryTracingBridge::new(&logger_provider).with_filter(log_filter);
        let fmt_filter =
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
        let fmt_layer = match config.log_format {
            LogFormat::Text => tracing_subscriber::fmt::layer().with_filter(fmt_filter).boxed(),
            LogFormat::Json => tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(true)
                .with_filter(fmt_filter)
                .boxed(),
        };
        tracing_subscriber::registry()
            .with(trace_layer)
            .with(log_layer)
            .with(fmt_layer)
            .try_init()
            .map_err(|error| TelemetryError::Subscriber(error.to_string()))?;

        let health = ExporterHealth {
            endpoint,
            protocol: "http/protobuf",
            configured_sample_ratio: config.sample_ratio,
            effective_sample_ratio: config.effective_sample_ratio(),
            incident_mode: config.incident_mode,
            initialized: true,
        };
        Ok(Self { tracer_provider, meter_provider, logger_provider, health })
    }

    pub fn health(&self) -> &ExporterHealth {
        &self.health
    }

    pub fn shutdown(self) -> Result<(), TelemetryError> {
        self.tracer_provider.shutdown().map_err(|error| TelemetryError::Flush {
            signal: "trace",
            message: error.to_string(),
        })?;
        self.logger_provider
            .shutdown()
            .map_err(|error| TelemetryError::Flush { signal: "log", message: error.to_string() })?;
        self.meter_provider.shutdown().map_err(|error| TelemetryError::Flush {
            signal: "metric",
            message: error.to_string(),
        })?;
        Ok(())
    }
}

pub fn emit_payload_event(evidence: &PayloadEvidence<'_>) -> bool {
    let context = tracing::Span::current().context();
    if !context.span().is_recording() {
        return false;
    }
    tracing::info!(
        name = "braid.payload",
        github.comment.body = evidence.github_body,
        github.comment.summary = evidence.github_summary,
        credential = evidence.credential,
        provider.transcript = evidence.provider_transcript,
        github.webhook.payload = evidence.webhook_payload,
        local.path = evidence.local_path,
        "sampled full-payload diagnostic evidence"
    );
    true
}

pub fn run_probe(config: &TelemetryConfig, marker: &str) -> Result<ProbeResult, TelemetryError> {
    let telemetry = TelemetryGuard::install(config, "probe")?;
    let root = tracing::info_span!("braid.telemetry.probe", marker);
    let sampled = root.context().span().is_recording();
    let payload_emitted = {
        let _entered = root.enter();
        let child = tracing::info_span!("braid.telemetry.probe.child");
        let _child_entered = child.enter();
        emit_payload_event(&PayloadEvidence {
            github_body: marker,
            github_summary: &format!("summary:{marker}"),
            credential: &format!("credential:{marker}"),
            provider_transcript: &format!("transcript:{marker}"),
            webhook_payload: &format!("webhook:{marker}"),
            local_path: &format!("/diagnostic/{marker}"),
        })
    };
    let meter = telemetry.meter_provider.meter("braid");
    meter
        .u64_counter("braid.telemetry.probes")
        .with_description("Public telemetry probe invocations")
        .build()
        .add(1, &[KeyValue::new("sampled", sampled)]);
    let result = ProbeResult { sampled, payload_emitted, exporter: telemetry.health().clone() };
    drop(root);
    telemetry.shutdown()?;
    Ok(result)
}
