// This constructor exists only in the integration-test compilation of github.rs.
// Production client construction and endpoint selection remain unchanged.
use super::*;
impl GitHubClient {
    #[allow(dead_code)]
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
