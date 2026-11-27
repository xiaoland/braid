//! Real CLI subprocess contracts; store and HTTP contracts live with their owners.
use std::fs;

#[test]
fn cli_exits_offline_with_queryable_receipt_and_no_remote_credentials() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("braid-cli-publication-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let example = include_str!("../config.example.toml");
    fs::write(&path, example.replace("/absolute/path/to/instance", dir.to_str().unwrap())).unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_braid"))
            .args(args)
            .arg("--config")
            .arg(&path)
            .env_remove("BRAID_AGENT")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output
    };
    run(&["migrate", "apply"]);
    // Fixed wire-format fixture: the CLI must validate a digest independently
    // of its own BodySnapshot serializer.
    let snapshot_file = dir.join("snapshot.json");
    fs::write(&snapshot_file, r#"{"target":"owner/repository#1","digest":"654f4b4e1aab7eab9d8087cec18f7c5d0a5424cfda1a516feb436b277683196a","snapshot":{"id":1,"node_id":"item","title":"Title","body":"Closes #2\n\nImplementation request: durable-marker","state":"open","updated_at":"version-1","pull_request":{}}}"#).unwrap();
    let body_file = dir.join("body.md");
    fs::write(&body_file, "Implementation scope and verification").unwrap();
    let edited = run(&[
        "gh",
        "pr",
        "edit",
        "owner/repository#1",
        "--operator",
        "--profile",
        "pr-codex",
        "--snapshot",
        snapshot_file.to_str().unwrap(),
        "--body-file",
        body_file.to_str().unwrap(),
        "--request-id",
        "offline-body",
        "--json",
    ]);
    let edited: serde_json::Value = serde_json::from_slice(&edited.stdout).unwrap();
    assert_eq!(edited["state"], "queued");
    assert_eq!(edited["operation"], "body_edit");
    assert_eq!(edited["role"], "Operator");
    let status = run(&["gh", "status", edited["request_id"].as_str().unwrap(), "--json"]);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&status.stdout).unwrap()["state"],
        "queued"
    );
    let output = run(&[
        "gh",
        "comment",
        "create",
        "--operator",
        "owner/repository#1",
        "--profile",
        "issue-codex",
        "--request-id",
        "offline",
        "--body",
        "Offline update",
        "--json",
    ]);
    let receipt: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(receipt["state"], "queued");
    assert!(receipt.get("intent_id").is_none());
    let id = receipt["request_id"].as_str().unwrap();
    let output = run(&["gh", "status", id, "--json"]);
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["request_id"], id);
    assert_eq!(status["state"], "queued");
    let output = run(&[
        "gh",
        "comment",
        "create",
        "--operator",
        "owner/repository#1",
        "--profile",
        "issue-codex",
        "--request-id",
        "offline",
        "--body",
        "Offline update",
        "--json",
    ]);
    let duplicate: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(duplicate["request_id"], id);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn gh_read_runs_on_existing_runtime_without_nested_runtime_panic() {
    use std::process::Command;
    let dir = std::env::temp_dir().join(format!("braid-cli-read-{}-", uuid::Uuid::now_v7()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let example = include_str!("../config.example.toml");
    fs::write(&path, example.replace("/absolute/path/to/instance", dir.to_str().unwrap())).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_braid"))
        .args(["gh", "pr", "read", "owner/repository#1"])
        .arg("--output")
        .arg(dir.join("snapshot.json"))
        .arg("--config")
        .arg(&path)
        .env_remove("BRAID_AGENT")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("Cannot start a runtime from within a runtime"),
        "nested runtime panic: {stderr}"
    );
    assert!(!output.status.success(), "read must fail without GitHub credentials");
    assert!(stderr.contains("cannot read GitHub App private key"), "wrong failure: {stderr}");
    assert!(output.status.code() != Some(101), "cli panicked: {stderr}");
    fs::remove_dir_all(dir).unwrap();
}
