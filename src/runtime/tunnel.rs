use super::*;

pub(crate) async fn signed_public_probe(
    url: &str,
    secret: &[u8],
    repository: &str,
    repository_node_id: &str,
) -> Result<()> {
    let body = serde_json::to_vec(&serde_json::json!({
        "zen":"Braid signed public tunnel probe",
        "repository":{"full_name":repository,"node_id":repository_node_id}
    }))?;
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&body);
    let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .tls_backend_rustls()
        .build()
        .context("cannot construct public tunnel probe client")?;
    let mut last = None;
    for _ in 0..18 {
        match client
            .post(url)
            .header("X-Hub-Signature-256", &signature)
            .header("X-GitHub-Delivery", Uuid::now_v7().to_string())
            .header("X-GitHub-Event", "ping")
            .header("Content-Type", "application/json")
            .body(body.clone())
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return Ok(()),
            Ok(response) => last = Some(format!("HTTP {}", response.status())),
            Err(error) => last = Some(format!("{error:?}")),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    bail!(
        "signed public tunnel probe did not converge: {}",
        last.as_deref().unwrap_or("no response")
    )
}

pub(crate) async fn start_verified_quick_tunnel(
    config: &Config,
    local_url: &str,
    secret: &[u8],
    repository: &str,
    repository_node_id: &str,
) -> Result<(QuickTunnel, String)> {
    let mut last_error = None;
    for attempt in 1..=3 {
        let started = match QuickTunnel::start(&config.tools.wrangler, local_url).await {
            Ok(started) => started,
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(attempt, error = %message, "cannot start Quick Tunnel candidate");
                last_error = Some(message);
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        let public_webhook = format!("{}/webhook", started.url);
        match signed_public_probe(&public_webhook, secret, repository, repository_node_id).await {
            Ok(()) => return Ok((started, public_webhook)),
            Err(error) => {
                let message = error.to_string();
                tracing::warn!(attempt, url = %public_webhook, error = %message, "discarding an unreachable Quick Tunnel");
                last_error = Some(message);
                if let Err(stop_error) = started.stop().await {
                    tracing::warn!(attempt, error = %stop_error, "cannot stop unreachable Quick Tunnel");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    bail!(
        "no verified Quick Tunnel became reachable after 3 candidates: {}",
        last_error.as_deref().unwrap_or("no public probe result")
    )
}

pub async fn probe_public_webhook(config: &Config, url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url).context("public webhook URL is invalid")?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none_or(|host| !host.ends_with(".trycloudflare.com"))
        || parsed.path() != "/webhook"
    {
        bail!("public webhook URL must be an HTTPS trycloudflare.com /webhook endpoint");
    }
    let secret = config
        .webhook_secret()
        .with_context(|| "cannot load GitHub webhook secret from configured source")?;
    if secret.is_empty() {
        bail!("GitHub webhook secret must not be empty");
    }
    let repository = config.github.repository.parse::<RepositoryName>()?;
    let github = GitHubClient::connect(&config.github, &repository).await?;
    signed_public_probe(
        url,
        secret.as_bytes(),
        &config.github.repository,
        &github.identity().repository_node_id,
    )
    .await
}

pub(crate) async fn restore_webhook(github: &GitHubClient, prior: &AppWebhookConfig) -> Result<()> {
    github.update_app_webhook(&prior.url, None).await?;
    Ok(())
}
