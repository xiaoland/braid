#![allow(clippy::wildcard_imports)]
use super::*;

pub fn migrate(command: MigrateCommand) -> Result<()> {
    let (arguments, apply) = match command {
        MigrateCommand::Plan(arguments) => (arguments, false),
        MigrateCommand::Apply(arguments) => (arguments, true),
    };
    let config = helpers::load(&arguments.config)?;
    let actor = helpers::store(&config)?;
    if apply {
        let result = actor.apply()?;
        if arguments.json {
            helpers::print_json(&result)?;
        } else {
            helpers::print_migration_result(&result);
        }
    } else {
        let plan = actor.plan()?;
        if arguments.json {
            helpers::print_json(&plan)?;
        } else {
            helpers::print_migration_plan(&plan);
        }
    }
    Ok(())
}
