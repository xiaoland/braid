use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WriteRequest {
    pub request_id: String,
    pub client_id: String,
    pub repository: String,
    pub target_kind: String,
    pub target_database_id: String,
    pub operation: String,
    pub content: String,
    pub remote_database_id: Option<String>,
}
