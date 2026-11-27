// Owner-local tests use a loopback HTTP client while retaining production
// request, permission, rate-limit and recovery behavior.
use super::*;
impl GitHubClient {
    pub fn publication_test_client(
        client: Octocrab,
        identity: GitHubIdentity,
        config: GitHubConfig,
    ) -> Self {
        let client = Arc::new(client);
        Self {
            app: Arc::clone(&client),
            installation: client,
            config,
            identity,
            projects_v2_enabled: false,
            rate_limits: Arc::new(RateLimits::default()),
            background: false,
        }
    }
}
