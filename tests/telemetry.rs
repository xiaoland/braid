#![allow(dead_code)]
mod config {
    #[derive(Clone, Copy)]
    pub enum LogFormat {
        Text,
        Json,
    }
    pub struct TelemetryConfig {
        pub endpoint: url::Url,
        pub service_name: String,
        pub export_timeout_seconds: u64,
        pub sample_ratio: f64,
        pub incident_mode: bool,
        pub log_format: LogFormat,
    }
    impl TelemetryConfig {
        pub fn effective_sample_ratio(&self) -> f64 {
            self.sample_ratio
        }
    }
}
#[path = "../src/telemetry.rs"]
mod telemetry;

use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_proto::tonic::collector::{
    logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
    trace::v1::ExportTraceServiceRequest,
};
use prost::Message as _;
use std::{
    io::{Read as _, Write as _},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

const SENTINEL: &str = "PRIVATE_PAYLOAD_SENTINEL_";
const RESPONSE_SENTINEL: &str = "PRIVATE_COLLECTOR_RESPONSE_SENTINEL";

#[test]
fn telemetry_child() {
    let Ok(endpoint) = std::env::var("BRAID_TEST_OTLP") else {
        return;
    };
    let config = config::TelemetryConfig {
        endpoint: endpoint.parse().unwrap(),
        service_name: "braid-test".into(),
        export_timeout_seconds: 1,
        sample_ratio: std::env::var("BRAID_TEST_RATIO").unwrap().parse().unwrap(),
        incident_mode: false,
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
    telemetry::bind_provider_turn("session-test", "native-turn-test");
    for stream in ["request", "stdout", "stderr"] {
        assert_eq!(
            telemetry::emit_provider_payload(
                "pi",
                "session-test",
                Some("native-turn-test"),
                stream,
                &payload
            ),
            config.sample_ratio > 0.5
        );
    }
    let root = tracing::info_span!("test.root");
    let sampled = root.context().span().is_recording();
    {
        let _entered = root.enter();
        assert_eq!(
            telemetry::emit_payload_event(&telemetry::PayloadEvidence {
                github_body: &payload,
                github_summary: &payload,
                credential: &payload,
                provider_transcript: &payload,
                webhook_payload: &payload,
                local_path: &payload,
            }),
            sampled
        );
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

#[test]
fn telemetry_routes_complete_evidence_only_to_otlp() {
    for format in ["text", "json"] {
        for ratio in ["0", "1"] {
            verify(format, ratio, false, false, "trace");
        }
    }
    verify("json", "1", false, true, "trace");
}

// Also run with --features opentelemetry-otlp/internal-logs to exercise its raw
// response_body diagnostic, which is disabled in the production feature set.
#[test]
fn telemetry_isolates_collector_rejection_bodies_from_ordinary_diagnostics() {
    for format in ["text", "json"] {
        for rust_log in ["debug", "trace", "trace,opentelemetry-otlp=trace,opentelemetry_sdk=trace"]
        {
            verify(format, "1", true, true, rust_log);
            verify(format, "1", true, false, rust_log);
        }
    }
}

type Requests = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

fn verify(format: &str, ratio: &str, fail: bool, probe: bool, rust_log: &str) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let requests: Requests = Arc::default();
    let done = Arc::new(AtomicBool::new(false));
    let received = Arc::clone(&requests);
    let stop = Arc::clone(&done);
    let server = std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let Ok((mut socket, _)) = listener.accept() else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
            let mut buffer = Vec::new();
            let header_end = loop {
                let mut chunk = [0; 8192];
                let count = socket.read(&mut chunk).unwrap();
                assert!(count > 0);
                buffer.extend_from_slice(&chunk[..count]);
                if let Some(offset) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                    break offset + 4;
                }
            };
            let headers = String::from_utf8_lossy(&buffer[..header_end]);
            let path = headers.split_whitespace().nth(1).unwrap().to_owned();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length:")
                        .map(|value| value.trim().parse().unwrap())
                })
                .unwrap();
            while buffer.len() < header_end + length {
                let mut chunk = [0; 8192];
                let count = socket.read(&mut chunk).unwrap();
                assert!(count > 0);
                buffer.extend_from_slice(&chunk[..count]);
            }
            received.lock().unwrap().push((path, buffer[header_end..].to_vec()));
            // reqwest rejects 4xx/5xx before OTLP can inspect the body. A 300
            // without Location reaches OTLP's non-2xx StatusError diagnostic.
            let status = if fail { "300 Multiple Choices" } else { "200 OK" };
            // Rejections can echo credentials or the original evidence payload.
            let response = if fail {
                format!("{RESPONSE_SENTINEL}: credential={SENTINEL}echoed")
            } else {
                String::new()
            };
            write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/x-protobuf\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        }
    });
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "telemetry_child", "--nocapture"])
        .env("BRAID_TEST_OTLP", &endpoint)
        .env("BRAID_TEST_FORMAT", format)
        .env("BRAID_TEST_RATIO", ratio)
        .env("RUST_LOG", rust_log);
    if probe {
        command.env("BRAID_TEST_PROBE", "1");
    }
    let output = command.output().unwrap();
    done.store(true, Ordering::Relaxed);
    server.join().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(!stderr.contains(SENTINEL), "payload leaked to {format} stderr ({rust_log})");
    assert!(
        !stderr.contains(RESPONSE_SENTINEL),
        "collector response leaked to {format} stderr ({rust_log})"
    );
    if !probe {
        assert!(stderr.contains("ordinary diagnostic after evidence"), "{stderr}");
    }
    if fail {
        assert!(!stderr.contains("HttpClient.StatusError"), "{stderr}");
        assert!(!stderr.contains("opentelemetry_sdk"), "{stderr}");
        assert!(stderr.contains("stage=sdk_export outcome=failed"), "{stderr}");
        for signal in ["logs", "traces", "metrics"] {
            assert!(stderr.contains(&format!("signal={signal}")), "{stderr}");
        }
        if probe {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let result: serde_json::Value =
                stdout.lines().find_map(|line| serde_json::from_str(line).ok()).unwrap();
            let signals = result["signals"].as_array().unwrap();
            assert_eq!(signals.len(), 3);
            for signal in signals {
                assert_eq!(signal["failed_batches"], 1);
                assert!(signal["failed_records"].as_u64().unwrap() > 0);
                assert_eq!(signal["successful_batches"], 0);
                assert!(signal["queue_dropped_records"].is_null());
            }
        }
    }
    verify_otlp(&requests.lock().unwrap(), ratio, probe);
}

fn verify_otlp(requests: &[(String, Vec<u8>)], ratio: &str, probe: bool) {
    let mut log_records = Vec::new();
    let mut span_ids = Vec::new();
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
                ExportMetricsServiceRequest::decode(body.as_slice()).unwrap();
            }
            other => panic!("unexpected signal {other}"),
        }
    }
    let payload = format!("{SENTINEL}{}END", "x".repeat(100_000));
    let mut payload_records = 0;
    for record in log_records {
        assert!(
            !serde_json::to_string(&record).unwrap().contains(RESPONSE_SENTINEL),
            "collector response recursed into OTLP logs"
        );
        let matches: Vec<_> = record.attributes.iter().filter(|attribute| {
            attribute.value.as_ref().is_some_and(|value| matches!(&value.value, Some(opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(text)) if text.contains(SENTINEL)))
        }).collect();
        if matches.is_empty() {
            continue;
        }
        payload_records += 1;
        assert_eq!(ratio, "1");
        assert!(
            span_ids.contains(&(record.trace_id.clone(), record.span_id.clone())),
            "payload missing exported parent"
        );
        for attribute in matches {
            let text = serde_json::to_value(attribute.value.as_ref().unwrap()).unwrap().to_string();
            assert!(text.contains(&payload), "incomplete {}", attribute.key);
        }
        if record.attributes.iter().any(|attribute| attribute.key == "provider.stream")
            && !record.attributes.iter().any(|attribute| attribute.key == "provider.execution.id")
        {
            for key in ["braid.provider_session.id", "braid.assignment.id", "braid.turn.id"] {
                assert!(record.attributes.iter().any(|attribute| attribute.key == key));
            }
        }
    }
    assert_eq!(
        payload_records,
        if ratio == "0" {
            0
        } else if probe {
            1
        } else {
            6
        }
    );
    if ratio == "0" {
        assert!(span_ids.is_empty());
    }
    if probe {
        assert!(requests.iter().any(|(path, _)| path == "/v1/metrics"));
    }
}
