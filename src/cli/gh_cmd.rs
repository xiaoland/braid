#![allow(clippy::wildcard_imports)]
use super::*;

pub fn gh(command: GhCommand) -> Result<()> {
    match command {
        GhCommand::Pr { command: GhPrCommand::Handoff(arguments) } => gh_handoff(arguments),
        GhCommand::Pr { command: GhPrCommand::Read(arguments) }
        | GhCommand::Issue { command: GhBodyCommand::Read(arguments) } => {
            let config = helpers::load(&arguments.source.resolve_config_path()?)?;
            let target = arguments.target.parse::<WorkItemLocator>()?;
            // `cli::run` already executes on the `#[tokio::main]` runtime, so a
            // nested `Runtime::new().block_on` panics; park this worker thread
            // and drive the read on the existing runtime instead.
            let snapshot = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(writer::read_for_edit(&config, &target))
            })?;
            fs::write(&arguments.output, snapshot)?;
            println!("canonical snapshot: {}", arguments.output.display());
            Ok(())
        }
        GhCommand::Pr { command: GhPrCommand::Edit(arguments) }
        | GhCommand::Issue { command: GhBodyCommand::Edit(arguments) } => {
            let config = helpers::load(&arguments.source.resolve_config_path()?)?;
            let actor = helpers::store(&config)?;
            let receipt = writer::edit(
                &config,
                &actor,
                writer::EditRequest {
                    invocation: arguments.invocation.resolve(),
                    target: arguments.target.parse()?,
                    profile_id: arguments.profile,
                    snapshot: fs::read_to_string(arguments.snapshot)?,
                    title: arguments.title,
                    body: arguments.body_file.map(fs::read_to_string).transpose()?,
                    request_id: arguments.request_id,
                },
            )?;
            print_gh_receipt(&receipt, arguments.json, &actor)
        }
        GhCommand::Status(arguments) => {
            let config = helpers::load(&arguments.source.resolve_config_path()?)?;
            if !arguments.request_id.starts_with("request-") {
                bail!("expected a public request ID");
            }
            let receipt = helpers::store(&config)?
                .gh_write_receipt(arguments.request_id)?
                .context("unknown publication request")?;
            print_gh_receipt(&receipt, arguments.json, &helpers::store(&config)?)
        }
        GhCommand::Comment { command: GhCommentCommand::Create(arguments) } => {
            let config_path = arguments.source.resolve_config_path()?;
            let config = helpers::load(&config_path)?;
            let target = arguments.target.parse::<WorkItemLocator>()?;
            let body = match (arguments.body, arguments.body_file) {
                (Some(body), None) => body,
                (None, Some(path)) => fs::read_to_string(&path).with_context(|| {
                    format!("cannot read Agent comment body {}", path.display())
                })?,
                (None, None) => bail!("comment create requires --body or --body-file"),
                (Some(_), Some(_)) => unreachable!("clap enforces body argument conflicts"),
            };
            let actor = helpers::store(&config)?;
            let receipt = writer::create_comment(
                &config,
                &actor,
                CommentCreateRequest {
                    invocation: arguments.invocation.resolve(),
                    target,
                    profile_id: arguments.profile,
                    body,
                    request_id: arguments.request_id,
                },
            )?;
            print_gh_receipt(&receipt, arguments.json, &helpers::store(&config)?)
        }
        GhCommand::Pr { command: GhPrCommand::Ensure(arguments) } => {
            let config_path = arguments.source.resolve_config_path()?;
            let config = helpers::load(&config_path)?;
            let actor = helpers::store(&config)?;
            let repository = match arguments.repository {
                Some(repository) => repository.parse::<RepositoryName>()?,
                None if config.is_multi_repository() => {
                    bail!("pr ensure requires --repository in a multi-repository instance")
                }
                None => config.repository().parse::<RepositoryName>()?,
            };
            let receipt = writer::ensure_pull_request(
                &config,
                &actor,
                PullRequestEnsureRequest {
                    invocation: arguments.invocation.resolve(),
                    repository,
                    comment_id: arguments.comment,
                    head: arguments.head,
                },
            )?;
            print_gh_receipt(&receipt, arguments.json, &helpers::store(&config)?)
        }
    }
}

fn print_gh_receipt(
    receipt: &crate::store::GhWriteReceipt,
    json: bool,
    store: &crate::store::StoreActor,
) -> Result<()> {
    let plan = store.implementation_request_receipt(receipt.intent_id.clone())?;
    if json {
        helpers::print_json(&CommentWriteResult {
            request_id: &receipt.request_id,
            operation: &receipt.operation,
            stage: plan.as_ref().map(|plan| plan.stage.as_str()),
            activation: plan.as_ref().map(|plan| activation_delivery(plan)),
            head: plan.as_ref().map(|plan| plan.head_ref.as_str()),
            base: plan.as_ref().map(|plan| plan.base_ref.as_str()),
            pull_request: plan
                .as_ref()
                .and_then(|plan| plan.pull_request_number)
                .map(|number| format!("https://github.com/{}/pull/{number}", receipt.repository)),
            state: if receipt.lifecycle == "pending" { "queued" } else { &receipt.lifecycle },
            target: &receipt.target,
            profile: &receipt.profile_id,
            role: &receipt.role,
            comment: receipt
                .remote_database_id
                .as_ref()
                .filter(|_| receipt.operation == "comment_create")
                .map(|id| format!("{}#issuecomment-{id}", receipt.repository)),
            error: receipt.last_error.as_deref(),
        })?;
    } else {
        println!("request ID: {}", receipt.request_id);
        println!("query: braid gh status {}", receipt.request_id);
        println!("operation: {}", receipt.operation);
        println!(
            "state: {}",
            if receipt.lifecycle == "pending" { "queued" } else { &receipt.lifecycle }
        );
        println!("target: {}", receipt.target);
        println!("Profile: {} ({})", receipt.profile_id, receipt.role);
        if let Some(plan) = &plan {
            println!("stage: {}", plan.stage);
            println!(
                "activation delivery: {} (not execution completion)",
                activation_delivery(plan)
            );
            println!("head: {} / base: {}", plan.head_ref, plan.base_ref);
            if let Some(number) = plan.pull_request_number {
                println!("PR: https://github.com/{}/pull/{number}", receipt.repository);
            }
        }
        if receipt.operation == "comment_create"
            && let Some(id) = &receipt.remote_database_id
        {
            println!("GitHub comment: {}#issuecomment-{id}", receipt.repository);
        }
        if let Some(error) = &receipt.last_error {
            println!("error: {error}");
        }
    }
    Ok(())
}

fn activation_delivery(plan: &crate::store::ImplementationRequestReceipt) -> &'static str {
    if plan.activation_delivered {
        "delivered"
    } else if plan.write.lifecycle == "applied" {
        "missing_requires_explicit_activation"
    } else {
        "pending"
    }
}

fn gh_handoff(arguments: GhPrHandoff) -> Result<()> {
    let config = helpers::load(&arguments.source.resolve_config_path()?)?;
    let actor = helpers::store(&config)?;
    let receipt = writer::handoff(
        &config,
        &actor,
        &writer::HandoffRequest {
            invocation: writer::Invocation::Agent(arguments.source_turn),
            target: arguments.target.parse()?,
            request_id: arguments.request_id,
        },
    )?;
    print_gh_receipt(&receipt, arguments.json, &actor)
}
