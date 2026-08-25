#![allow(clippy::wildcard_imports)]
use super::*;

pub fn status(arguments: &ConfigPath) -> Result<()> {
    let config = helpers::load(&arguments.config)?;
    let database = helpers::store(&config)?.status()?;
    let transport = if database.pending_migrations == 0 {
        Some(helpers::store(&config)?.runtime_status()?)
    } else {
        None
    };
    let status = LocalStatus {
        binary_version: env!("CARGO_PKG_VERSION"),
        config_schema: config.schema_version,
        repository: &config.github.repository,
        default_pr_profile: &config.profile_selection.default_pr_profile,
        database,
        telemetry_endpoint: config.telemetry.endpoint.to_string(),
        telemetry_sample_ratio: config.telemetry.effective_sample_ratio(),
        telemetry_incident_mode: config.telemetry.incident_mode,
        transport,
    };
    if arguments.json {
        helpers::print_json(&status)?;
    } else {
        println!("braid {}", status.binary_version);
        println!("repository: {}", status.repository);
        println!(
            "database schema: {}/{} ({} pending)",
            status.database.schema_version,
            status.database.supported_schema,
            status.database.pending_migrations
        );
        println!("database: {}", status.database.database.display());
        println!("OTLP: {} (ratio {})", status.telemetry_endpoint, status.telemetry_sample_ratio);
        if let Some(transport) = &status.transport {
            println!(
                "transport: deliveries={} duplicates={} pending={} runnable={} writes={}/{} resets={}",
                transport.deliveries,
                transport.duplicate_deliveries,
                transport.pending_batches,
                transport.runnable_batches,
                transport.pending_writes,
                transport.uncertain_writes,
                transport.context_resets.len()
            );
        }
    }
    Ok(())
}
