#![allow(clippy::wildcard_imports)]
use super::*;

pub fn load(path: &Path) -> Result<Config> {
    if !path.is_absolute() {
        bail!("--config must be an absolute path: {}", path.display());
    }
    Config::load(path).with_context(|| format!("configuration rejected: {}", path.display()))
}

pub fn store(config: &Config) -> Result<StoreActor> {
    StoreActor::start(
        config.runtime.database().to_path_buf(),
        config.runtime.backups().to_path_buf(),
    )
    .context("cannot start SQLite actor")
}

pub fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

pub fn print_migration_plan(plan: &MigrationPlan) {
    println!("database: {}", plan.database.display());
    println!("schema: {}/{}", plan.current_schema, plan.supported_schema);
    if plan.pending.is_empty() {
        println!("pending: none");
    } else {
        for migration in &plan.pending {
            println!("pending: {:04} {} {}", migration.version, migration.name, migration.checksum);
        }
    }
}

pub fn print_migration_result(result: &MigrationResult) {
    println!("schema: {} -> {}", result.previous_schema, result.current_schema);
    println!(
        "applied: {}",
        result.applied.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
    );
    if let Some(backup) = &result.backup {
        println!("backup: {}", backup.display());
    }
}

pub async fn tunnel_probe(arguments: TunnelProbe) -> Result<()> {
    let config = load(&arguments.config)?;
    crate::tunnel::probe_public_webhook(&config, &arguments.url).await?;
    println!("public webhook probe: accepted");
    Ok(())
}
