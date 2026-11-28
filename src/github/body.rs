//! Canonical body snapshot and narrow title/body mutation; no transport policy here.
use super::{GitHubClient, GitHubError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BodySnapshot {
    pub id: u64,
    pub node_id: String,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    pub updated_at: String,
    pub pull_request: Option<serde_json::Value>,
}
impl BodySnapshot {
    pub fn kind(&self) -> &'static str {
        if self.pull_request.is_some() { "pr" } else { "issue" }
    }
    pub fn digest(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("snapshot serializes");
        hex::encode(Sha256::digest(bytes))
    }
}
impl GitHubClient {
    pub async fn body_snapshot(&self, number: u64) -> Result<BodySnapshot, GitHubError> {
        self.write_rest(
            "GET",
            &format!("/repos/{}/issues/{number}", self.identity().repository),
            None::<&()>,
        )
        .await
    }
    pub async fn edit_body(
        &self,
        number: u64,
        title: &str,
        body: &str,
    ) -> Result<BodySnapshot, GitHubError> {
        #[derive(Serialize)]
        struct Edit<'a> {
            title: &'a str,
            body: &'a str,
        }
        self.write_rest(
            "PATCH",
            &format!("/repos/{}/issues/{number}", self.identity().repository),
            Some(&Edit { title, body }),
        )
        .await
    }
}
