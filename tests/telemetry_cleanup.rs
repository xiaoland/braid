#![allow(dead_code)]
use serde_json::Value;
use std::{path::PathBuf, time::Duration};
use tokio::{process::Command, time::timeout};

struct PiConfig {
    native_runtime: Option<PathBuf>,
    executable: PathBuf,
}
#[derive(Debug, thiserror::Error)]
enum ProviderError {
    #[error("{0}")]
    Protocol(String),
    #[error("{0}")]
    Unknown(String),
    #[error("{method}")]
    Timeout { method: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
mod telemetry {
    pub fn emit_provider_cleanup(_: &str, _: &[u8], _: &[u8]) {}
}
mod provider_runtime {
    include!("../src/provider/pi_runtime.rs");

    #[tokio::test]
    async fn cleanup_receipt_still_requires_every_stop_proof_and_never_errors_with_raw_output() {
        let directory = std::env::temp_dir().join(format!("braid-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let receipt = serde_json::json!({
            "status": "stopped", "execution_id": "execution-test", "scope": "execution",
            "coverage": "registered-process-groups-and-inherited-execution-markers",
            "private": "PRIVATE_CLEANUP_SENTINEL"
        });
        for case in ["valid", "exit", "status", "execution_id", "scope", "coverage", "malformed"] {
            let mut candidate = receipt.clone();
            if !matches!(case, "valid" | "exit" | "malformed") {
                candidate[case] = Value::String("wrong".into());
            }
            let stdout = if case == "malformed" {
                "PRIVATE_CLEANUP_SENTINEL".to_owned()
            } else {
                candidate.to_string()
            };
            let module = directory.join("cleanup.sh");
            std::fs::write(
                &module,
                format!(
                    "printf '%s' '{stdout}'\nprintf '%s' 'PRIVATE_CLEANUP_SENTINEL' >&2\nexit {}\n",
                    u8::from(case == "exit")
                ),
            )
            .unwrap();
            let execution = Execution {
                pi_version: "test".into(),
                id: "execution-test".into(),
                directory: directory.clone(),
                extensions: BTreeSet::new(),
                node: "/bin/sh".into(),
                module,
                owner: PathBuf::new(),
                start_id: "test".into(),
            };
            let result = execution.cleanup().await;
            if case == "valid" {
                result.unwrap();
            } else {
                let error = result.unwrap_err().to_string();
                assert!(!error.contains("PRIVATE_CLEANUP_SENTINEL"), "{case}: {error}");
                assert!(error.contains("execution-test"), "{error}");
                assert!(error.contains("stage="), "{error}");
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }
}
