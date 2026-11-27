use super::*;

/// This verifies the native cleanup receipt boundary, not that the fixture
/// actually stopped processes. Native process ownership has its own Node tests.
#[cfg(unix)]
#[tokio::test]
async fn cleanup_receipt_requires_every_stop_proof_without_exposing_raw_output() {
    let directory = std::env::temp_dir().join(format!("braid-cleanup-{}", uuid::Uuid::now_v7()));
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
            continue;
        }
        let error = result.unwrap_err();
        if case == "malformed" {
            assert!(
                matches!(&error, ProviderError::Protocol(message) if message.contains("stage=decode"))
            );
        } else {
            assert!(
                matches!(&error, ProviderError::Unknown(message) if message.contains("stage=validate_receipt")),
                "{case}: {error}"
            );
        }
        let diagnostic = error.to_string();
        assert!(!diagnostic.contains("PRIVATE_CLEANUP_SENTINEL"), "{case}: {diagnostic}");
        assert!(diagnostic.contains("execution-test"), "{diagnostic}");
    }
    std::fs::remove_dir_all(directory).unwrap();
}
