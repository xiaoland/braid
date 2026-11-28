use super::*;
use serde_json::{Value, json};

fn configuration(
    root: &Path,
    kind: CredentialKind,
    executable: Option<&Path>,
    manifest: Option<&Path>,
) -> Config {
    let mut config: Config = toml::from_str(include_str!("../../config.example.toml")).unwrap();
    for runtime in &mut config.runtimes {
        runtime.adapter_type = "pi".into();
        runtime.version = "1.1.0".into();
        runtime.home = None;
        if let Some(executable) = executable {
            runtime.executable = executable.to_owned();
        }
        runtime.native_runtime = manifest.map(Path::to_owned);
    }
    for profile in &mut config.profiles {
        profile.adapter_type = "pi".into();
        profile.adapter_version = "1.1.0".into();
        profile.provider = Some("test-llm".into());
        profile.model = Some("test-chat".into());
        profile.workspace = Some(root.to_owned());
    }
    let mut provider: LlmProvider = toml::from_str(
        r#"
id = "test-llm"
base_url = "https://llm.example.invalid/v1"
api = "openai-completions"
[[models]]
id = "test-chat"
name = "Test Chat"
reasoning = true
input = ["text", "image"]
context_window = 32000
max_tokens = 2048
thinking_level_map = { high = "enabled" }
compat = { supports_store = false, max_tokens_field = "max_completion_tokens" }
"#,
    )
    .unwrap();
    let secret = root.join("credential.toml");
    let contents = match kind {
        CredentialKind::ApiKey => Some("provider_api_key = \"PRIVATE_API_KEY\"\n"),
        CredentialKind::OAuth => Some(
            "access = \"PRIVATE_ACCESS\"\nrefresh = \"PRIVATE_REFRESH\"\nexpires = 4102444800000\n",
        ),
        CredentialKind::None => None,
    };
    if let Some(contents) = contents {
        fs::write(&secret, contents).unwrap();
    }
    provider.credential = Some(ProviderCredential { kind, file: contents.map(|_| secret) });
    config.llm_providers = vec![provider];
    let path = root.join("config.toml");
    fs::write(&path, toml::to_string(&config).unwrap()).unwrap();
    Config::load(&path).unwrap()
}

#[test]
fn snapshot_writes_native_model_fields_and_keeps_credentials_out_of_models_json() {
    let root = std::env::temp_dir().join(format!("braid-pi-models-{}", uuid::Uuid::now_v7()));
    for (name, kind) in [("api-key", CredentialKind::ApiKey), ("oauth", CredentialKind::OAuth)] {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        let config = configuration(&directory, kind, None, None);
        let profile = &config.profiles[0];
        let materials = collect(&config, profile).unwrap();
        let home_key = uuid::Uuid::now_v7().to_string();
        let home = snapshot(&config, profile, &directory, &home_key, &materials.digest).unwrap();
        let actual: Value =
            serde_json::from_slice(&fs::read(home.join("models.json")).unwrap()).unwrap();
        let mut expected = json!({"providers":{"test-llm":{
            "baseUrl":"https://llm.example.invalid/v1", "api":"openai-completions",
            "models":[{"id":"test-chat","name":"Test Chat","reasoning":true,
                "input":["text","image"],"contextWindow":32000,"maxTokens":2048,
                "thinkingLevelMap":{"high":"enabled"},
                "compat":{"supportsStore":false,"maxTokensField":"max_completion_tokens"}}]
        }}});
        if kind == CredentialKind::ApiKey {
            expected["providers"]["test-llm"]["apiKey"] = json!("$BRAID_PROVIDER_API_KEY");
        }
        assert_eq!(actual, expected, "{name}");
        if kind == CredentialKind::ApiKey {
            // Resume uses this same frozen home; it must not silently overwrite a
            // changed model route just because the identity file still matches.
            fs::write(home.join("models.json"), br#"{"providers":{}}"#).unwrap();
            let error =
                snapshot(&config, profile, &directory, &home_key, &materials.digest).unwrap_err();
            assert!(
                error.to_string().contains("Pi native material snapshot changed at"),
                "{error}"
            );
            assert_eq!(fs::read(home.join("models.json")).unwrap(), br#"{"providers":{}}"#);
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "installed Pi distribution; run with BRAID_TEST_PI_INSTALL and PI_OFFLINE=1"]
async fn failed_home_binding_stops_the_installed_execution_before_returning() {
    let install = PathBuf::from(std::env::var_os("BRAID_TEST_PI_INSTALL").unwrap());
    let root = std::env::temp_dir().join(format!("braid-pi-bind-{}", uuid::Uuid::now_v7()));
    let homes = root.join(".braid/pi-homes");
    fs::create_dir_all(&homes).unwrap();
    // A real post-launch metadata write fails after the SDK has opened its session.
    fs::write(homes.join("index"), "not a directory").unwrap();
    let config = configuration(
        &root,
        CredentialKind::None,
        Some(&install.join("pi")),
        Some(&install.join("runtime-manifest.json")),
    );
    let profile = config.profiles[0].clone();
    let factories = crate::provider::session_factories(&config).unwrap();
    let factory = &factories[&profile.id];
    let result = factory.start(profile, String::new(), "initial context".into()).await;

    let mut executions = fs::read_dir(root.join(".braid/native-executions")).unwrap();
    let execution = executions.next().unwrap().unwrap();
    assert!(executions.next().is_none());
    let id = execution.file_name();
    let receipt = fs::read(execution.path().join("execution-stop.json"));
    if receipt.is_err() {
        // Reclaim only this test's execution when an old/broken adapter abandons it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let cleanup = tokio::process::Command::new(install.join("bin/node"))
                .arg(install.join("managed.mjs"))
                .args(["cleanup", "--execution-dir"])
                .arg(execution.path())
                .arg("--execution-id")
                .arg(&id)
                .output()
                .await
                .unwrap();
            assert!(cleanup.status.success());
            let cleanup: Value = serde_json::from_slice(&cleanup.stdout).unwrap();
            if cleanup["status"] == "stopped" {
                break;
            }
            // Child exit and OS marker inspection can overlap on the abandoned execution.
            assert!(
                std::time::Instant::now() < deadline,
                "test teardown could not confirm stop: {cleanup}"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
    assert!(
        matches!(result, Err(crate::agent_session::SessionError::Failed(ref reason))
        if reason.contains("cannot bind Pi session native home")),
        "{:?}",
        result.err()
    );
    let receipt: Value = serde_json::from_slice(
        &receipt.expect("failed factory construction omitted the owned-stop receipt"),
    )
    .unwrap();
    assert_eq!(receipt["execution_id"], id.to_str().unwrap());
    assert_eq!(receipt["scope"], "execution");
    assert_eq!(receipt["status"], "stopped");
    assert_eq!(receipt["coverage"], "registered-process-groups-and-inherited-execution-markers");
}

#[cfg(unix)]
#[tokio::test]
async fn startup_rejection_retains_an_unconfirmed_physical_shutdown() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = std::env::temp_dir().join(format!("braid-pi-exit-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&root).unwrap();
    let executable = root.join("pi");
    fs::write(
        &executable,
        r#"#!/bin/sh
IFS= read -r request || exit 20
case "$request" in *'"new_session"'*) ;; *) exit 21 ;; esac
id=$(printf '%s' "$request" | sed -n 's/.*"id"[[:space:]]*:[[:space:]]*\([0-9][0-9]*\).*/\1/p')
printf '{"id":%s,"success":false,"error":"session creation rejected"}\n' "$id"
if IFS= read -r extra; then exit 22; fi
exit 29
"#,
    )
    .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let config = configuration(&root, CredentialKind::None, Some(&executable), None);
    let profile = config.profiles[0].clone();
    let factories = crate::provider::session_factories(&config).unwrap();
    let factory = &factories[&profile.id];
    let error = factory.start(profile, String::new(), String::new()).await.err().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        matches!(error, crate::agent_session::SessionError::Unknown(ref cause)
        if cause.contains("session creation rejected")
            && cause.contains("External Pi parent exited with exit status: 29")),
        "{error}"
    );
}
