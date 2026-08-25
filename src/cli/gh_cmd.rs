#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn gh(command: GhCommand) -> Result<()> {
    match command {
        GhCommand::Comment { command: GhCommentCommand::Create(arguments) } => {
            let config = helpers::load(&arguments.config)?;
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
                    target,
                    profile_id: arguments.profile,
                    body,
                    request_id: arguments.request_id,
                },
            )
            .await?;
            print_gh_receipt(&receipt, arguments.json)
        }
        GhCommand::Pr { command: GhPrCommand::Ensure(arguments) } => {
            let config = helpers::load(&arguments.config)?;
            let actor = helpers::store(&config)?;
            let receipt = writer::ensure_pull_request(
                &config,
                &actor,
                PullRequestEnsureRequest { comment_id: arguments.comment, head: arguments.head },
            )
            .await?;
            if arguments.json {
                helpers::print_json(&PullRequestEnsureResult {
                    operation: "pr_ensure",
                    state: &receipt.write.lifecycle,
                    stage: &receipt.stage,
                    implementation_request_comment: receipt.comment_database_id,
                    issue: format!("{}#{}", receipt.write.repository, receipt.issue_number),
                    base: &receipt.base_ref,
                    head: &receipt.head_ref,
                    profile: &receipt.pr_profile_id,
                    pull_request: receipt
                        .pull_request_number
                        .map(|number| format!("{}#{number}", receipt.write.repository)),
                    error: receipt.write.last_error.as_deref(),
                })?;
            } else {
                println!("state: {} / {}", receipt.write.lifecycle, receipt.stage);
                println!("Implementation Request: {}", receipt.write.target);
                println!("head: {}", receipt.head_ref);
                if let Some(number) = receipt.pull_request_number {
                    println!("PR: {}#{number}", receipt.write.repository);
                }
                println!("PR Profile: {}", receipt.pr_profile_id);
            }
            Ok(())
        }
    }
}

fn print_gh_receipt(receipt: &crate::store::GhWriteReceipt, json: bool) -> Result<()> {
    if json {
        helpers::print_json(&CommentWriteResult {
            operation: &receipt.operation,
            state: &receipt.lifecycle,
            target: &receipt.target,
            profile: &receipt.profile_id,
            role: &receipt.role,
            comment: receipt
                .remote_database_id
                .as_ref()
                .map(|id| format!("{}#issuecomment-{id}", receipt.repository)),
            error: receipt.last_error.as_deref(),
        })?;
    } else {
        println!("operation: {}", receipt.operation);
        println!("state: {}", receipt.lifecycle);
        println!("target: {}", receipt.target);
        println!("Profile: {} ({})", receipt.profile_id, receipt.role);
        if let Some(id) = &receipt.remote_database_id {
            println!("GitHub comment: {}#issuecomment-{id}", receipt.repository);
        }
        if let Some(error) = &receipt.last_error {
            println!("error: {error}");
        }
    }
    Ok(())
}
