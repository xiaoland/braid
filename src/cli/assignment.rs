use anyhow::{Context as _, Result};

use super::{AssignmentReactivate, helpers};

pub fn reactivate(arguments: AssignmentReactivate) -> Result<()> {
    let config_path = arguments.source.resolve_config_path()?;
    let config = helpers::load(&config_path)?;
    let repository = arguments.repository.unwrap_or_else(|| config.repository().to_owned());
    if !config.repositories().contains(&repository.as_str()) {
        anyhow::bail!(
            "repository {repository:?} is not configured for this instance (configured: {})",
            config.repositories().join(", ")
        );
    }
    let store = crate::store::StoreActor::start(
        config.runtime.database().to_path_buf(),
        config.runtime.backups().to_path_buf(),
    )
    .context("cannot open the Braid ledger for assignment reactivation")?;
    let event_id = store.enqueue_blocked_reactivation(
        repository.clone(),
        arguments.kind,
        arguments.number,
        arguments.reason.unwrap_or_default(),
        crate::store::SchedulerPolicy {
            quiet_seconds: config.scheduler.quiet_seconds,
            event_threshold: config.scheduler.event_threshold,
        },
    )?;
    println!(
        "Queued blocked-assignment reactivation {event_id} for {repository}#{}.\nThe running or next Harness will materialize a new generation with the current effective Profile; the old blocked generation remains unchanged.",
        arguments.number
    );
    Ok(())
}
