#![allow(clippy::wildcard_imports)]
use super::*;

pub async fn github(command: GitHubCommand) -> Result<()> {
    match command {
        GitHubCommand::Probe(arguments) => github_probe(arguments).await,
        GitHubCommand::Webhook(arguments) => {
            let config = helpers::load(&arguments.resolve_config_path()?)?;
            let repository = config.github.repository.parse::<RepositoryName>()?;
            let client = GitHubClient::connect(&config.github, &repository).await?;
            let webhook = client.app_webhook_config().await?;
            if arguments.json {
                helpers::print_json(&webhook)?;
            } else {
                println!("URL: {}", webhook.url);
                println!("content type: {}", webhook.content_type);
                println!("insecure SSL: {}", webhook.insecure_ssl);
            }
            Ok(())
        }
        GitHubCommand::Deliveries(arguments) => {
            let config = helpers::load(&arguments.resolve_config_path()?)?;
            let repository = config.github.repository.parse::<RepositoryName>()?;
            let client = GitHubClient::connect(&config.github, &repository).await?;
            let deliveries = client.app_deliveries().await?;
            if arguments.json {
                helpers::print_json(&deliveries)?;
            } else {
                for delivery in deliveries {
                    println!(
                        "{}\t{}\t{}\t{}\t{}",
                        delivery.id,
                        delivery.guid,
                        delivery.event,
                        delivery.action.as_deref().unwrap_or(""),
                        delivery.status
                    );
                }
            }
            Ok(())
        }
        GitHubCommand::Redeliver(arguments) => {
            let config = helpers::load(&arguments.config)?;
            let repository = config.github.repository.parse::<RepositoryName>()?;
            let client = GitHubClient::connect(&config.github, &repository).await?;
            client.redeliver(arguments.delivery_id).await?;
            println!("redelivery requested: {}", arguments.delivery_id);
            Ok(())
        }
    }
}

async fn github_probe(arguments: GitHubProbe) -> Result<()> {
    let config = helpers::load(&arguments.config)?;
    let repository = arguments.repository.parse::<RepositoryName>()?;
    let client =
        GitHubClient::connect(&config.github, &repository).await.context("GitHub probe failed")?;
    if arguments.json {
        helpers::print_json(client.identity())?;
    } else {
        let identity = client.identity();
        println!("GitHub App: {} ({})", identity.app_slug, identity.app_id);
        println!("installation: {}", identity.installation_id);
        println!("repository: {}", identity.repository);
        println!("repository node: {}", identity.repository_node_id);
        println!("App actor: {} ({})", identity.actor_login, identity.actor_node_id);
        println!("token expires: {}", identity.token_expires_at);
        println!(
            "permissions: {}",
            identity
                .permissions
                .iter()
                .map(|(name, level)| format!("{name}={level}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}
