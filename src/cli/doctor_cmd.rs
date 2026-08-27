#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn doctor(arguments: &ConfigPath) -> Result<()> {
    let config = helpers::load(&arguments.resolve_config_path()?)?;
    let report = doctor::run(&config).await;
    if arguments.json {
        helpers::print_json(&report)?;
    } else {
        for check in &report.checks {
            println!("{:?}\t{}\t{}", check.state, check.name, check.detail);
        }
    }
    if !report.ready {
        bail!("doctor found failing or unavailable preconditions");
    }
    Ok(())
}
