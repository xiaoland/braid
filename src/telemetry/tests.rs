use crate::{config, telemetry};

use axum::{
    Router,
    body::Bytes,
    http::{StatusCode, Uri, header},
    routing::post,
};
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use prost::Message as _;
use std::sync::mpsc;
use tokio::process::Command;

const SENTINEL: &str = "PRIVATE_PAYLOAD_SENTINEL_";
const RESPONSE_SENTINEL: &str = "PRIVATE_COLLECTOR_RESPONSE_SENTINEL";

#[test]
#[ignore = "subprocess entry; exercised by the two parent telemetry tests"]
fn telemetry_child() {
    let endpoint = std::env::var("BRAID_TEST_OTLP").expect("parent provides OTLP endpoint");
    let config = config::TelemetryConfig {
        endpoint: endpoint.parse().unwrap(),
        service_name: "braid-test".into(),
        export_timeout_seconds: 1,
        sample_ratio: std::env::var("BRAID_TEST_RATIO").unwrap().parse().unwrap(),
        incident_mode: std::env::var("BRAID_TEST_INCIDENT").unwrap() == "1",
        log_format: if std::env::var("BRAID_TEST_FORMAT").unwrap() == "json" {
            config::LogFormat::Json
        } else {
            config::LogFormat::Text
        },
    };
    let payload = format!("{SENTINEL}{}END", "x".repeat(100_000));
    if std::env::var("BRAID_TEST_PROBE").is_ok() {
        let result = telemetry::run_probe(&config, &payload).unwrap();
        println!("{}", serde_json::to_string(&result).unwrap());
        return;
    }
    let guard = telemetry::TelemetryGuard::install(&config, "test").unwrap();
    telemetry::set_runtime_generation(Some("generation-test".into()));
    telemetry::register_session(telemetry::SessionIdentity {
        work_item_kind: "issue".into(),
        work_item_number: 42,
        assignment_id: "assignment-test".into(),
        assignment_generation: 1,
        provider_session_id: "session-test".into(),
        provider_kind: "pi".into(),
        context_revision: "context-test".into(),
        profile_id: "profile-test".into(),
        profile_revision: 1,
        profile_digest: "digest-test".into(),
    });
    telemetry::begin_turn("session-test", "turn-test");
    telemetry::emit_provider_payload("pi", "session-test", None, "request", &payload);
    telemetry::bind_provider_turn("session-test", "native-turn-test");
    telemetry::emit_provider_payload(
        "pi",
        "session-test",
        Some("native-turn-test"),
        "stdout",
        &payload,
    );
    let root = tracing::info_span!("test.root");
    {
        let _entered = root.enter();
        telemetry::emit_payload_event(&telemetry::PayloadEvidence {
            github_body: &payload,
            github_summary: &format!("summary:{payload}"),
            credential: &format!("credential:{payload}"),
            provider_transcript: &format!("transcript:{payload}"),
            webhook_payload: &format!("webhook:{payload}"),
            local_path: &format!("/diagnostic/{payload}"),
        });
        telemetry::emit_provider_cleanup("execution-test", payload.as_bytes(), payload.as_bytes());
        // Evidence-target spans must not leak through fmt's inherited context.
        let secret = tracing::info_span!(target: "braid::evidence", "evidence.test", credential = payload.as_str());
        let _secret = secret.enter();
        tracing::info!("ordinary diagnostic after evidence");
    }
    drop(root);
    telemetry::set_runtime_generation(None);
    guard.shutdown().unwrap();
}

#[tokio::test]
async fn telemetry_routes_complete_evidence_only_to_otlp() {
    verify(Scenario::Runtime, "text", "0", StatusCode::OK, false).await;
    verify(Scenario::Incident, "json", "0", StatusCode::OK, true).await;
}

// The internal-logs CI feature makes the SDK emit its raw rejection diagnostic.
#[tokio::test]
async fn telemetry_isolates_collector_rejection_bodies_from_ordinary_diagnostics() {
    // A redirect without Location reaches OTLP's error diagnostic with the body intact.
    verify(Scenario::Runtime, "json", "1", StatusCode::MULTIPLE_CHOICES, true).await;
    verify(Scenario::Probe, "text", "1", StatusCode::MULTIPLE_CHOICES, true).await;
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Runtime,
    Incident,
    Probe,
}

async fn verify(scenario: Scenario, format: &str, ratio: &str, status: StatusCode, retained: bool) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let (received, requests) = mpsc::channel();
    let probe = scenario == Scenario::Probe;
    let app = Router::new().route(
        "/v1/{signal}",
        post(move |uri: Uri, body: Bytes| {
            let status = if probe && uri.path() == "/v1/metrics" { StatusCode::OK } else { status };
            received.send((uri.path().to_owned(), body.to_vec())).unwrap();
            async move {
                let response = if status.is_success() { "" } else { RESPONSE_SENTINEL };
                (status, [(header::CONTENT_TYPE, "application/x-protobuf")], response)
            }
        }),
    );
    let server = tokio::spawn(axum::serve(listener, app).into_future());
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "telemetry::tests::telemetry_child", "--nocapture"])
        .env("BRAID_TEST_OTLP", &endpoint)
        .env("BRAID_TEST_FORMAT", format)
        .env("BRAID_TEST_RATIO", ratio)
        .env("BRAID_TEST_INCIDENT", if scenario == Scenario::Incident { "1" } else { "0" })
        .env_remove("BRAID_TEST_PROBE")
        .env("RUST_LOG", "trace,opentelemetry-otlp=trace,opentelemetry_sdk=trace");
    if probe {
        command.env("BRAID_TEST_PROBE", "1");
    }
    let output = command.output().await.unwrap();
    server.abort();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains(SENTINEL), "payload leaked to {format} stderr");
    assert!(!stderr.contains(RESPONSE_SENTINEL), "collector response leaked to {format} stderr");
    if !probe {
        assert!(stderr.contains("ordinary diagnostic after evidence"), "{stderr}");
    }
    if !status.is_success() {
        assert!(!stderr.contains("HttpClient.StatusError"), "{stderr}");
        assert!(!stderr.contains("opentelemetry_sdk"), "{stderr}");
        assert!(stderr.contains("stage=sdk_export outcome=failed"), "{stderr}");
        for signal in ["logs", "traces", "metrics"] {
            let failed = format!("signal={signal} stage=sdk_export outcome=failed");
            assert_eq!(stderr.contains(&failed), !probe || signal != "metrics", "{stderr}");
        }
        if probe {
            verify_probe(&output.stdout);
        }
    }
    verify_otlp(&requests.try_iter().collect::<Vec<_>>(), retained, probe);
}

fn verify_probe(output: &[u8]) {
    let stdout = String::from_utf8_lossy(output);
    let result: serde_json::Value =
        stdout.lines().find_map(|line| serde_json::from_str(line).ok()).unwrap();
    let signals = result["signals"].as_array().unwrap();
    assert_eq!(signals.len(), 3);
    for (signal, succeeded) in [("logs", false), ("traces", false), ("metrics", true)] {
        let report = signals.iter().find(|report| report["signal"] == signal).unwrap();
        assert_eq!(report["successful_batches"], u64::from(succeeded), "{signal}");
        assert_eq!(report["failed_batches"], u64::from(!succeeded), "{signal}");
        assert_eq!(report["failed_records"].as_u64().unwrap() > 0, !succeeded, "{signal}");
        assert!(report["queue_dropped_records"].is_null(), "{signal}");
    }
}

fn verify_otlp(requests: &[(String, Vec<u8>)], retained: bool, probe: bool) {
    let mut log_records = Vec::new();
    let mut span_ids = Vec::new();
    let mut metrics = Vec::new();
    for (path, body) in requests {
        match path.as_str() {
            "/v1/logs" => {
                for resource in
                    ExportLogsServiceRequest::decode(body.as_slice()).unwrap().resource_logs
                {
                    for scope in resource.scope_logs {
                        assert!(
                            !scope.scope.as_ref().unwrap().name.starts_with("opentelemetry"),
                            "raw SDK/exporter diagnostics recursed into OTLP logs"
                        );
                        log_records.extend(scope.log_records);
                    }
                }
            }
            "/v1/traces" => {
                for resource in
                    ExportTraceServiceRequest::decode(body.as_slice()).unwrap().resource_spans
                {
                    for scope in resource.scope_spans {
                        for span in scope.spans {
                            assert!(
                                !serde_json::to_string(&span).unwrap().contains(SENTINEL),
                                "evidence duplicated into spans"
                            );
                            span_ids.push((span.trace_id, span.span_id));
                        }
                    }
                }
            }
            "/v1/metrics" => {
                for resource in
                    ExportMetricsServiceRequest::decode(body.as_slice()).unwrap().resource_metrics
                {
                    for scope in resource.scope_metrics {
                        metrics.extend(scope.metrics);
                    }
                }
            }
            other => panic!("unexpected signal {other}"),
        }
    }
    if !retained {
        use opentelemetry_proto::tonic::common::v1::any_value::Value;

        assert!(span_ids.is_empty());
        assert!(
            log_records.iter().any(|record| matches!(
                record.body.as_ref().and_then(|body| body.value.as_ref()),
                Some(Value::StringValue(message)) if message == "session evidence sampling decision"
            )),
            "ordinary session observation must reach OTLP even with evidence sampling disabled"
        );
    }
    verify_payload_records(log_records, &span_ids, retained, probe);
    let counter = if probe { "braid.telemetry.probes" } else { "braid.runtime.transitions" };
    let metric = metrics
        .iter()
        .find(|metric| metric.name == counter)
        .expect("counter exported even with trace sampling disabled");
    let Some(opentelemetry_proto::tonic::metrics::v1::metric::Data::Sum(sum)) = &metric.data else {
        panic!("{counter} is not a counter");
    };
    assert!(sum.data_points.iter().any(|point| matches!(point.value, Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(value)) if value > 0)));
}

fn verify_payload_records(
    log_records: Vec<opentelemetry_proto::tonic::logs::v1::LogRecord>,
    span_ids: &[(Vec<u8>, Vec<u8>)],
    retained: bool,
    probe: bool,
) {
    let payload = format!("{SENTINEL}{}END", "x".repeat(100_000));
    let mut expected_streams: std::collections::BTreeSet<&str> = if retained && !probe {
        ["request", "stdout", "cleanup.stdout", "cleanup.stderr"].into()
    } else {
        std::collections::BTreeSet::new()
    };
    let mut payload_events = 0;
    for record in log_records {
        let serialized = serde_json::to_string(&record).unwrap();
        if !retained {
            assert!(
                !serialized.contains(SENTINEL),
                "unsampled payload exported in OTLP log record"
            );
        }
        assert!(
            !serialized.contains(RESPONSE_SENTINEL),
            "collector response recursed into OTLP logs"
        );
        let fields: std::collections::BTreeMap<_, _> = record
            .attributes
            .iter()
            .filter_map(|field| {
                use opentelemetry_proto::tonic::common::v1::any_value::Value;
                match field.value.as_ref()?.value.as_ref()? {
                    Value::StringValue(text) => Some((field.key.as_str(), text.as_str())),
                    _ => None,
                }
            })
            .collect();
        if !fields.values().any(|value| value.contains(SENTINEL)) {
            continue;
        }
        assert!(retained);
        assert!(
            span_ids.contains(&(record.trace_id.clone(), record.span_id.clone())),
            "payload missing exported parent"
        );
        if let Some(&stream) = fields.get("provider.stream") {
            assert!(expected_streams.remove(stream), "duplicate or unexpected stream {stream}");
            assert!(
                fields.get("provider.transcript") == Some(&payload.as_str()),
                "{stream} incomplete"
            );
            if stream.starts_with("cleanup.") {
                assert_eq!(fields.get("provider.execution.id"), Some(&"execution-test"));
            } else {
                for (key, expected) in [
                    ("braid.provider_session.id", "session-test"),
                    ("braid.assignment.id", "assignment-test"),
                    ("braid.turn.id", "turn-test"),
                ] {
                    assert_eq!(fields.get(key), Some(&expected), "{stream}: {key}");
                }
            }
        } else {
            payload_events += 1;
            for (field, prefix) in [
                ("github.comment.body", ""),
                ("github.comment.summary", "summary:"),
                ("credential", "credential:"),
                ("provider.transcript", "transcript:"),
                ("github.webhook.payload", "webhook:"),
                ("local.path", "/diagnostic/"),
            ] {
                assert!(
                    fields.get(field).and_then(|value| value.strip_prefix(prefix))
                        == Some(payload.as_str()),
                    "missing, incomplete, or misrouted {field}"
                );
            }
        }
    }
    assert!(expected_streams.is_empty(), "missing streams: {expected_streams:?}");
    assert_eq!(payload_events, usize::from(retained));
}
