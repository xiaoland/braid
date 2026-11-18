#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn telemetry_probe(arguments: TelemetryProbe) -> Result<()> {
    let config_path = arguments.source.resolve_config_path()?;
    let config = helpers::load(&config_path)?;
    let telemetry_config = config.telemetry;
    let marker = arguments.marker;
    let result =
        tokio::task::spawn_blocking(move || telemetry::run_probe(&telemetry_config, &marker))
            .await
            .context("telemetry worker panicked")??;
    if arguments.json {
        helpers::print_json(&result)?;
    } else {
        println!("sampled: {}", result.sampled);
        println!("payload emitted: {}", result.payload_emitted);
        println!("OTLP endpoint: {}", result.exporter.endpoint);
        println!("trace ID: {}", result.trace_id);
        for signal in &result.signals {
            println!(
                "{} {}: SDK batches successful={}, failed={} (not a persistence acknowledgement)",
                signal.signal, signal.endpoint, signal.successful_batches, signal.failed_batches
            );
        }
        println!("Verify logs/traces/metrics in the backend; see docs/40-deployment/README.md.");
    }
    Ok(())
}
