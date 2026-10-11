use std::time::{Duration, Instant};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

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

#[derive(Debug, Clone, Serialize)]
pub struct SessionIdentity {
    pub work_item_kind: String,
    pub work_item_number: u64,
    pub assignment_id: String,
    pub assignment_generation: u64,
    pub provider_session_id: String,
    pub provider_kind: String,
    pub context_revision: String,
}

struct ObservedSession {
    identity: SessionIdentity,
    span: tracing::Span,
    pending_turn: Option<String>,
    turns: BTreeMap<String, ObservedTurn>,
}

struct ObservedTurn {
    braid_turn: String,
    span: Option<tracing::Span>,
    accepted_at: Option<Instant>,
}

struct RuntimeMetrics {
    transitions: opentelemetry::metrics::Counter<u64>,
    lifecycle: opentelemetry::metrics::Counter<u64>,
    turn_elapsed: opentelemetry::metrics::Histogram<f64>,
    payloads: opentelemetry::metrics::Counter<u64>,
}

static RUNTIME_METRICS: OnceLock<RuntimeMetrics> = OnceLock::new();

fn install_runtime_metrics(provider: &SdkMeterProvider) {
    let meter = provider.meter("braid.runtime");
    RUNTIME_METRICS.get_or_init(|| RuntimeMetrics {
        transitions: meter
            .u64_counter("braid.runtime.transitions")
            .with_description("Runtime observation generation boundaries; not execution stop proof")
            .build(),
        lifecycle: meter
            .u64_counter("braid.provider.lifecycle.events")
            .with_description("Observed provider lifecycle facts and input dispositions")
            .build(),
        turn_elapsed: meter
            .f64_histogram("braid.provider.turn.elapsed")
            .with_unit("s")
            .with_description("Monotonic elapsed time from accepted turn to observed terminal")
            .build(),
        payloads: meter
            .u64_counter("braid.provider.payloads")
            .with_description("Provider payload sampling and identity coverage")
            .build(),
    });
}

fn metric_event(event: &str) -> &'static str {
    match event {
        "prompt" => "prompt",
        "steer" => "steer",
        "terminal" => "terminal",
        "stop" => "stop",
        "prompt_response" => "prompt_response",
        _ => "other",
    }
}

fn metric_outcome(outcome: &str) -> &'static str {
    match outcome {
        "accepted" => "accepted",
        "deferred" => "deferred",
        "failed" => "failed",
        "unknown" => "unknown",
        "completed" => "completed",
        "interrupted" => "interrupted",
        "confirmed" => "confirmed",
        "unconfirmed" => "unconfirmed",
        "late_error" => "late_error",
        _ => "other",
    }
}

fn metric_provider(provider: &str) -> &'static str {
    match provider {
        "pi" => "pi",
        "codex" => "codex",
        _ => "unassigned",
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct EvidenceHealth {
    pub runtime_generation: Option<String>,
    pub registered_sessions: usize,
    pub emitted_payloads: u64,
    pub unsampled_payloads: u64,
    pub unassigned_payloads: u64,
}

#[derive(Default)]
struct ObservationState {
    health: EvidenceHealth,
    sessions: BTreeMap<String, ObservedSession>,
}

fn observations() -> &'static Mutex<ObservationState> {
    static STATE: OnceLock<Mutex<ObservationState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(ObservationState::default()))
}

pub fn set_runtime_generation(generation: Option<String>) {
    let mut state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if generation != state.health.runtime_generation
        && let Some(metrics) = RUNTIME_METRICS.get()
    {
        metrics.transitions.add(
            1,
            &[KeyValue::new(
                "outcome",
                if generation.is_some() { "generation_started" } else { "generation_ended" },
            )],
        );
    }
    state.sessions.clear();
    if generation.is_some() {
        state.health = EvidenceHealth { runtime_generation: generation, ..Default::default() };
    } else {
        state.health.runtime_generation = None;
    }
    tracing::info!(
        name = "braid.runtime.generation",
        braid.runtime.generation = state.health.runtime_generation.as_deref().unwrap_or(""),
        "runtime observation generation changed"
    );
}

pub fn evidence_health() -> EvidenceHealth {
    let state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    EvidenceHealth { registered_sessions: state.sessions.len(), ..state.health.clone() }
}

/// This read-only projection never determines runtime scheduling or recovery.
pub fn register_session_from_store(
    database: &Path,
    native_thread: &str,
) -> Result<(), rusqlite::Error> {
    let connection = rusqlite::Connection::open_with_flags(
        database,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut statement = connection.prepare(
        "SELECT w.kind,w.number,a.assignment_id,a.generation,ps.provider_session_id,ps.provider_kind,ps.context_revision
         FROM provider_sessions ps JOIN agent_instances ai ON ai.agent_id=ps.agent_id
         JOIN assignments a ON a.assignment_id=ai.assignment_id JOIN work_items w ON w.node_id=a.work_item_node_id
         WHERE ps.provider_session_id=?1")?;
    let identities = statement
        .query_map([native_thread], |row| {
            Ok(SessionIdentity {
                work_item_kind: row.get(0)?,
                work_item_number: observed_u64(row, 1)?,
                assignment_id: row.get(2)?,
                assignment_generation: observed_u64(row, 3)?,
                provider_session_id: row.get(4)?,
                provider_kind: row.get(5)?,
                context_revision: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if identities.len() != 1 {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "native session {native_thread} has {} durable identity rows; observation registration requires exactly one",
            identities.len()
        )));
    }
    register_session(identities.into_iter().next().expect("one identity"));
    Ok(())
}

fn observed_u64(row: &rusqlite::Row<'_>, index: usize) -> Result<u64, rusqlite::Error> {
    u64::try_from(row.get::<_, i64>(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

pub fn register_session(identity: SessionIdentity) {
    let mut state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if state.sessions.contains_key(&identity.provider_session_id) {
        return;
    }
    let span = tracing::info_span!(parent: None, "braid.provider.session",
        braid.runtime.generation = state.health.runtime_generation.as_deref().unwrap_or(""),
        braid.work_item.kind = %identity.work_item_kind, braid.work_item.number = identity.work_item_number,
        braid.assignment.id = %identity.assignment_id, braid.assignment.generation = identity.assignment_generation,
        braid.provider_session.id = %identity.provider_session_id, provider.thread.id = %identity.provider_session_id,
        provider.kind = %identity.provider_kind, braid.context.revision = %identity.context_revision);
    let entered = span.enter();
    tracing::info!(name = "braid.session.observation", braid.provider_session.id = %identity.provider_session_id,
        observation.sampled = span.context().span().is_recording(), "session evidence sampling decision");
    drop(entered);
    state.sessions.insert(
        identity.provider_session_id.clone(),
        ObservedSession { identity, span, pending_turn: None, turns: BTreeMap::new() },
    );
}

pub fn begin_turn(native_thread: &str, braid_turn: &str) {
    let mut state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(session) = state.sessions.get_mut(native_thread) {
        session.pending_turn = Some(braid_turn.to_owned());
    }
}

pub fn bind_provider_turn(native_thread: &str, native_turn: &str) {
    let mut state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(session) = state.sessions.get_mut(native_thread) {
        if session.turns.contains_key(native_turn) {
            return;
        }
        if let Some(braid_turn) = session.pending_turn.clone() {
            let span = tracing::info_span!(parent: &session.span, "braid.provider.turn",
                braid.turn.id = %braid_turn, provider.turn.id = native_turn,
                braid.provider_session.id = %session.identity.provider_session_id);
            session.turns.insert(
                native_turn.to_owned(),
                ObservedTurn { braid_turn, span: Some(span), accepted_at: None },
            );
        }
    }
}

fn observation_span(native_thread: &str, native_turn: Option<&str>) -> Option<tracing::Span> {
    let state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.sessions.get(native_thread).map(|session| {
        native_turn
            .and_then(|turn| session.turns.get(turn))
            .and_then(|turn| turn.span.as_ref())
            .unwrap_or(&session.span)
            .clone()
    })
}

fn observation_identity(
    native_thread: &str,
    native_turn: Option<&str>,
    pending: bool,
) -> (Option<SessionIdentity>, String) {
    let state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    state.sessions.get(native_thread).map_or((None, String::new()), |session| {
        let turn = native_turn
            .and_then(|id| session.turns.get(id))
            .map(|turn| turn.braid_turn.clone())
            .or_else(|| pending.then(|| session.pending_turn.clone()).flatten())
            .unwrap_or_default();
        (Some(session.identity.clone()), turn)
    })
}

pub fn lifecycle_event(
    native_thread: &str,
    native_turn: Option<&str>,
    event: &str,
    outcome: &str,
    reason: Option<&str>,
) {
    let span = observation_span(native_thread, native_turn);
    let entered = span.as_ref().map(tracing::Span::enter);
    let state = evidence_health();
    let (identity, braid_turn) =
        observation_identity(native_thread, native_turn, event == "prompt");
    let provider =
        identity.as_ref().map_or("unassigned", |identity| identity.provider_kind.as_str());
    if let Some(metrics) = RUNTIME_METRICS.get() {
        metrics.lifecycle.add(
            1,
            &[
                KeyValue::new("event", metric_event(event)),
                KeyValue::new("outcome", metric_outcome(outcome)),
                KeyValue::new("provider", metric_provider(provider)),
            ],
        );
    }
    tracing::info!(
        name = "braid.lifecycle",
        braid.runtime.generation = state.runtime_generation.as_deref().unwrap_or(""),
        braid.work_item.kind = identity.as_ref().map_or("", |i| i.work_item_kind.as_str()),
        braid.work_item.number = identity.as_ref().map(|i| i.work_item_number),
        braid.assignment.id = identity.as_ref().map_or("", |i| i.assignment_id.as_str()),
        braid.assignment.generation = identity.as_ref().map(|i| i.assignment_generation),
        braid.context.revision = identity.as_ref().map_or("", |i| i.context_revision.as_str()),
        braid.turn.id = braid_turn.as_str(),
        braid.provider_session.id = native_thread,
        provider.thread.id = native_thread,
        provider.turn.id = native_turn.unwrap_or(""),
        braid.event = event,
        braid.outcome = outcome,
        error.reason = reason.unwrap_or(""),
        observation.correlated = span.is_some(),
        "provider lifecycle observation"
    );
    drop(entered);
    if event == "terminal" || (event == "prompt" && outcome == "accepted") {
        let mut state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(turn) = native_turn.and_then(|turn| {
            state.sessions.get_mut(native_thread).and_then(|session| session.turns.get_mut(turn))
        }) {
            if event == "terminal" {
                turn.span.take();
                if let Some(accepted_at) = turn.accepted_at.take()
                    && let Some(metrics) = RUNTIME_METRICS.get()
                {
                    metrics.turn_elapsed.record(
                        accepted_at.elapsed().as_secs_f64(),
                        &[
                            KeyValue::new("provider", metric_provider(provider)),
                            KeyValue::new("outcome", metric_outcome(outcome)),
                        ],
                    );
                }
            } else {
                turn.accepted_at.get_or_insert_with(Instant::now);
            }
        }
    }
}

pub fn emit_provider_payload(
    provider: &str,
    native_thread: &str,
    native_turn: Option<&str>,
    stream: &str,
    payload: &str,
) -> bool {
    let span = observation_span(native_thread, native_turn);
    let correlated = span.is_some();
    let generation = evidence_health().runtime_generation;
    let span = span.unwrap_or_else(|| {
        tracing::info_span!(parent: None, "braid.provider.connection",
        provider.kind = provider, braid.runtime.generation = generation.as_deref().unwrap_or(""))
    });
    let sampled = span.context().span().is_recording();
    if let Some(metrics) = RUNTIME_METRICS.get() {
        metrics.payloads.add(
            1,
            &[
                KeyValue::new("provider", metric_provider(provider)),
                KeyValue::new("sampled", sampled),
                KeyValue::new("correlated", correlated),
            ],
        );
    }
    let (identity, braid_turn) =
        observation_identity(native_thread, native_turn, stream == "request");
    {
        let mut state = observations().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !correlated {
            state.health.unassigned_payloads += 1;
        }
        if sampled {
            state.health.emitted_payloads += 1;
        } else {
            state.health.unsampled_payloads += 1;
        }
    }
    let _entered = span.enter();
    if sampled {
        tracing::info!(
            name = "braid.provider.payload",
            provider.kind = provider,
            braid.runtime.generation = generation.as_deref().unwrap_or(""),
            braid.work_item.kind = identity.as_ref().map_or("", |i| i.work_item_kind.as_str()),
            braid.work_item.number = identity.as_ref().map(|i| i.work_item_number),
            braid.assignment.id = identity.as_ref().map_or("", |i| i.assignment_id.as_str()),
            braid.assignment.generation = identity.as_ref().map(|i| i.assignment_generation),
            braid.context.revision = identity.as_ref().map_or("", |i| i.context_revision.as_str()),
            braid.turn.id = braid_turn.as_str(),
            braid.provider_session.id = native_thread,
            provider.thread.id = native_thread,
            provider.turn.id = native_turn.unwrap_or(""),
            provider.stream = stream,
            provider.transcript = payload,
            observation.correlated = correlated,
            observation.sampled = true,
            "sampled complete provider evidence"
        );
    }
    sampled
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
        install_runtime_metrics(&meter_provider);

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
            // Logs are diagnostics and belong on stderr; stdout is reserved
            // for CLI results (e.g. `braid telemetry probe --json`).
            LogFormat::Text => tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(fmt_filter)
                .boxed(),
            LogFormat::Json => tracing_subscriber::fmt::layer()
                .json()
                .with_current_span(true)
                .with_writer(std::io::stderr)
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
