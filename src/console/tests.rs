//! Collector HTTP contracts. Inputs use the standard OTLP encoder, not Braid's decoder.
use super::{api, storage::Storage};
use opentelemetry_proto::tonic::{
    collector::{
        logs::v1::ExportLogsServiceRequest, metrics::v1::ExportMetricsServiceRequest,
        trace::v1::ExportTraceServiceRequest,
    },
    common::v1::{AnyValue, KeyValue, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    metrics::v1::{
        Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics, metric, number_data_point,
    },
    resource::v1::Resource,
    trace::v1::{ResourceSpans, ScopeSpans, Span},
};
use prost::Message as _;
use reqwest::StatusCode;
use serde_json::Value;
use std::{
    io::Write as _,
    path::PathBuf,
    sync::{Arc, Mutex},
};

struct Collector {
    root: PathBuf,
    url: String,
    client: reqwest::Client,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl Collector {
    async fn new() -> Self {
        let root = std::env::temp_dir().join(format!("braid-collector-{}", uuid::Uuid::now_v7()));
        let mut collector = Self {
            root,
            url: String::new(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
            stop: None,
            server: None,
        };
        collector.start().await;
        collector
    }

    async fn start(&mut self) {
        let storage =
            Arc::new(Mutex::new(Storage::open(&self.root.join("observations.sqlite3")).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        self.url = format!("http://{}", listener.local_addr().unwrap());
        let app = api::routes(api::ApiState { storage });
        let (stop, stopped) = tokio::sync::oneshot::channel();
        self.stop = Some(stop);
        self.server = Some(tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
                .unwrap();
        }));
    }

    async fn shutdown(&mut self) {
        self.stop.take().unwrap().send(()).unwrap();
        self.server.take().unwrap().await.unwrap();
    }

    async fn get(&self, path: &str) -> Value {
        self.client
            .get(format!("{}{path}", self.url))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap()
    }

    async fn export(&self, signal: &str, encoding: &str, body: &[u8]) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/{signal}", self.url))
            .header("content-type", "application/x-protobuf")
            .header("content-encoding", encoding)
            .body(body.to_vec())
            .send()
            .await
            .unwrap()
    }
}

impl Drop for Collector {
    fn drop(&mut self) {
        if let Some(server) = &self.server {
            server.abort();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn attribute(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue { value: Some(any_value::Value::StringValue(value.into())) }),
        ..KeyValue::default()
    }
}

fn resource() -> Resource {
    Resource {
        attributes: vec![
            attribute("service.instance.id", "instance-A"),
            attribute("service.name", "contract-service"),
            attribute("braid.session.id", "resource-session"),
        ],
        ..Resource::default()
    }
}

fn trace_body() -> Vec<u8> {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(resource()),
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![0x11; 16],
                    span_id: vec![0x22; 8],
                    name: "contract-span".into(),
                    start_time_unix_nano: 100,
                    end_time_unix_nano: 200,
                    attributes: vec![
                        attribute("braid.provider_session.id", "span-session"),
                        attribute("braid.turn.id", "turn-A"),
                    ],
                    ..Span::default()
                }],
                ..ScopeSpans::default()
            }],
            ..ResourceSpans::default()
        }],
    }
    .encode_to_vec()
}

fn log_body() -> Vec<u8> {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(resource()),
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    body: Some(AnyValue {
                        value: Some(any_value::Value::StringValue("retained evidence".into())),
                    }),
                    trace_id: vec![0x11; 16],
                    span_id: vec![0x22; 8],
                    time_unix_nano: 150,
                    attributes: vec![
                        attribute("braid.session.id", "log-session"),
                        attribute("braid.turn.id", "turn-B"),
                    ],
                    ..LogRecord::default()
                }],
                ..ScopeLogs::default()
            }],
            ..ResourceLogs::default()
        }],
    }
    .encode_to_vec()
}

fn metric_body() -> Vec<u8> {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            resource: Some(resource()),
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: "contract.gauge".into(),
                    data: Some(metric::Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: 150,
                            value: Some(number_data_point::Value::AsInt(42)),
                            ..NumberDataPoint::default()
                        }],
                    })),
                    ..Metric::default()
                }],
                ..ScopeMetrics::default()
            }],
            ..ResourceMetrics::default()
        }],
    }
    .encode_to_vec()
}

#[tokio::test]
async fn otlp_signals_raw_bytes_duplicates_and_cursor_pages_survive_reopening() {
    let mut collector = Collector::new().await;
    let trace = trace_body();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&log_body()).unwrap();
    let log = gzip.finish().unwrap();
    let metric = metric_body();
    for (signal, encoding, body) in [
        ("traces", "identity", trace.as_slice()),
        ("logs", "gzip", log.as_slice()),
        ("metrics", "identity", metric.as_slice()),
        ("traces", "identity", trace.as_slice()),
    ] {
        let response = collector.export(signal, encoding, body).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "application/x-protobuf");
        assert!(response.bytes().await.unwrap().is_empty());
    }
    let page = collector.get("/api/records?limit=2&instance=instance-A").await;
    let first = page["records"].as_array().unwrap();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0]["signal"], "traces");
    assert_eq!(first[0]["session"], "span-session");
    assert_eq!(first[0]["turn_id"], "turn-A");
    assert_eq!(first[0]["trace_id"], "11111111111111111111111111111111");
    assert_eq!(first[0]["data"]["name"], "contract-span");
    assert_eq!(first[1]["signal"], "logs");
    assert_eq!(first[1]["session"], "log-session");
    assert_eq!(first[1]["data"]["body"]["stringValue"], "retained evidence");
    for record in first {
        assert_eq!(record["service"], "contract-service");
        assert!(record["duplicate_of"].is_null());
    }
    let cursor = page["next_after"].as_i64().unwrap();
    let first_batch = first[0]["batch_id"].clone();
    let log_batch = first[1]["batch_id"].as_i64().unwrap();
    collector.shutdown().await;
    collector.start().await;
    assert_eq!(collector.get("/api/records?limit=2&instance=instance-A").await, page);
    let page =
        collector.get(&format!("/api/records?after={cursor}&limit=2&instance=instance-A")).await;
    let remaining = page["records"].as_array().unwrap();
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0]["signal"], "metrics");
    assert_eq!(remaining[0]["session"], "resource-session");
    assert_eq!(remaining[0]["data"]["name"], "contract.gauge");
    assert_eq!(remaining[0]["data"]["gauge"]["dataPoints"][0]["asInt"], 42);
    assert_eq!(remaining[1]["signal"], "traces");
    assert_eq!(remaining[1]["duplicate_of"], first_batch);
    let cursor = page["next_after"].as_i64().unwrap();
    assert!(
        collector.get(&format!("/api/records?after={cursor}")).await["records"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let filtered = collector.get("/api/records?signal=logs&session=log-session&turn=turn-B").await;
    assert_eq!(filtered["records"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["records"][0]["data"]["body"]["stringValue"], "retained evidence");
    let raw = collector
        .client
        .get(format!("{}/api/batches/{log_batch}", collector.url))
        .send()
        .await
        .unwrap();
    assert_eq!(raw.status(), StatusCode::OK);
    assert_eq!(raw.headers()["x-otlp-content-encoding"], "gzip");
    assert_eq!(raw.bytes().await.unwrap().as_ref(), log);
    let resources = collector.get("/api/resources").await;
    assert_eq!(resources["batches"], 4);
    assert_eq!(resources["records"], 4);
    assert_eq!(resources["duplicate_batches"], 1);
    collector.shutdown().await;
}

// google.rpc.Status field numbers are an independent HTTP error wire contract.
#[derive(prost::Message)]
struct ErrorStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

async fn expect_error(response: reqwest::Response, status: StatusCode, code: i32, reason: &str) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()["content-type"], "application/x-protobuf");
    let error = ErrorStatus::decode(response.bytes().await.unwrap()).unwrap();
    assert_eq!(error.code, code);
    assert!(error.message.contains(reason), "{}", error.message);
}

#[tokio::test]
async fn malformed_exports_reach_their_expected_guard_and_retain_rejected_bytes() {
    let mut collector = Collector::new().await;
    for (encoding, body, status, reason) in [
        ("br", vec![0xff], StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported Content-Encoding: br"),
        ("identity", vec![0xff], StatusCode::BAD_REQUEST, "decode OTLP logs protobuf"),
        ("gzip", vec![0xff], StatusCode::BAD_REQUEST, "decode gzip OTLP body"),
    ] {
        expect_error(collector.export("logs", encoding, &body).await, status, 3, reason).await;
    }
    let wrong_type = collector
        .client
        .post(format!("{}/v1/logs", collector.url))
        .header("content-type", "application/json")
        .body(log_body())
        .send()
        .await
        .unwrap();
    expect_error(wrong_type, StatusCode::UNSUPPORTED_MEDIA_TYPE, 3, "Content-Type").await;
    assert_eq!(collector.get("/api/resources").await["batches"], 0);
    let errors = collector.get("/api/receive-errors").await;
    assert_eq!(errors["errors"].as_array().unwrap().len(), 4);
    let id = errors["errors"][1]["id"].as_i64().unwrap();
    collector.shutdown().await;
    collector.start().await;
    assert_eq!(collector.get("/api/receive-errors").await, errors);
    let body = collector
        .client
        .get(format!("{}/api/receive-errors/{id}", collector.url))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.as_ref(), [0xff]);
    collector.shutdown().await;
}

#[tokio::test]
async fn record_storage_failure_is_retryable_and_does_not_leave_a_partial_batch() {
    let mut collector = Collector::new().await;
    let db = rusqlite::Connection::open(collector.root.join("observations.sqlite3")).unwrap();
    // Fail after the batch INSERT through SQLite itself, without replacing the storage owner.
    db.execute_batch("CREATE TRIGGER fail_record BEFORE INSERT ON console_records BEGIN SELECT RAISE(ABORT, 'injected record storage failure'); END;").unwrap();
    expect_error(
        collector.export("logs", "identity", &log_body()).await,
        StatusCode::SERVICE_UNAVAILABLE,
        14,
        "injected record storage failure",
    )
    .await;
    let resources = collector.get("/api/resources").await;
    assert_eq!(resources["records"], 0);
    assert_eq!(resources["batches"], 0);
    assert_eq!(resources["receive_errors"], 1);
    db.execute_batch("DROP TRIGGER fail_record").unwrap();
    drop(db);
    let response = collector.export("logs", "identity", &log_body()).await;
    assert_eq!(response.status(), StatusCode::OK);
    let records = collector.get("/api/records").await;
    assert_eq!(records["records"].as_array().unwrap().len(), 1);
    assert_eq!(records["records"][0]["data"]["body"]["stringValue"], "retained evidence");
    assert_eq!(collector.get("/api/resources").await["duplicate_batches"], 0);
    collector.shutdown().await;
}
