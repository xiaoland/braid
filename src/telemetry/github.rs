//! GraphQL-only measurements; REST and App-token requests are not observed here.
//! Metric attributes deliberately exclude installation, repository and object identities.
use opentelemetry::{KeyValue, metrics::MeterProvider as _};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use std::sync::OnceLock;

struct GitHubMetrics {
    requests: opentelemetry::metrics::Counter<u64>,
    latency: opentelemetry::metrics::Histogram<f64>,
    points: opentelemetry::metrics::Counter<u64>,
}
static METRICS: OnceLock<GitHubMetrics> = OnceLock::new();

pub(crate) fn install(provider: &SdkMeterProvider) {
    let meter = provider.meter("braid.github");
    METRICS.get_or_init(|| GitHubMetrics {
        requests: meter
            .u64_counter("braid.github.requests")
            .with_description("Admitted Braid GraphQL transport requests only; excludes REST and App-token requests")
            .build(),
        latency: meter
            .f64_histogram("braid.github.request.duration")
            .with_description("Braid GraphQL transport duration only; excludes REST and App-token requests")
            .with_unit("s")
            .build(),
        points: meter
            .u64_counter("braid.github.graphql.points")
            .with_description("Braid GraphQL only: server-returned rateLimit.cost; absence is unknown, not zero")
            .build(),
    });
}

pub(crate) fn observe(
    operation: &'static str,
    class: &'static str,
    outcome: &'static str,
    elapsed: f64,
    cost: Option<u64>,
) {
    let Some(metrics) = METRICS.get() else { return };
    let attributes = [
        KeyValue::new("operation", operation),
        KeyValue::new("class", class),
        KeyValue::new("resource", "graphql"),
        KeyValue::new("outcome", outcome),
        KeyValue::new("cost_known", cost.is_some()),
    ];
    metrics.requests.add(1, &attributes);
    metrics.latency.record(elapsed, &attributes);
    if let Some(cost) = cost {
        metrics.points.add(cost, &attributes);
    }
}
