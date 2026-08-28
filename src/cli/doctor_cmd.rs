#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn doctor(arguments: &ConfigPath) -> Result<()> {
    let config_path = arguments.resolve_config_path()?;
    let user_home = crate::home::UserHome::resolve(None)?;
    let config = helpers::load(&config_path)?;
    let report = doctor::run(&config, &user_home).await;
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
