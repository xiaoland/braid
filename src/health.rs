//! Operator-facing health snapshot. Leaf module shared by the runtime health
//! server (writer of `ready`/ingress fields, reader for `/health`) and the
//! workers (writers of provider/reconciliation/tunnel fields).

#[derive(Debug, Clone, serde::Serialize)]
pub struct HealthSnapshot {
    pub ready: bool,
    pub ingress: String,
    pub repository: String,
    pub tunnel: &'static str,
    pub webhook_url: Option<String>,
    pub reconciliation: &'static str,
    pub provider: &'static str,
    pub last_error: Option<String>,
}
