#![allow(clippy::wildcard_imports)]
use super::*;

pub fn config_check(arguments: &ConfigPath) -> Result<()> {
    let config = helpers::load(&arguments.resolve_config_path()?)?;
    if arguments.json {
        helpers::print_json(&config.summary())?;
    } else {
        println!("configuration: valid");
        println!("schema: {}", config.schema_version);
        println!("repository: {}", config.github.repository);
        println!("database: {}", config.runtime.database().display());
        println!(
            "profiles: {}",
            config
                .profiles
                .iter()
                .map(|profile| profile.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        println!("effective trace sample ratio: {}", config.telemetry.effective_sample_ratio());
    }
    Ok(())
}
