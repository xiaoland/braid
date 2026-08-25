#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn telemetry_probe(arguments: TelemetryProbe) -> Result<()> {
    let config = helpers::load(&arguments.config)?;
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
    }
    Ok(())
}
