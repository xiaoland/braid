use super::*;
use serde_json::{Value, json};

fn configuration(root: &Path, kind: CredentialKind) -> Config {
    let mut config: Config = toml::from_str(include_str!("../../config.example.toml")).unwrap();
    for runtime in &mut config.runtimes {
        runtime.adapter_type = "pi".into();
        runtime.version = "1.1.0".into();
        runtime.home = None;
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
    for (name, kind) in [
        ("api-key", CredentialKind::ApiKey),
        ("oauth", CredentialKind::OAuth),
        ("none", CredentialKind::None),
    ] {
        let directory = root.join(name);
        fs::create_dir_all(&directory).unwrap();
        let config = configuration(&directory, kind);
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
        // Resume uses this same frozen home; it must not silently overwrite a
        // changed model route just because the identity file still matches.
        fs::write(home.join("models.json"), br#"{"providers":{}}"#).unwrap();
        let error =
            snapshot(&config, profile, &directory, &home_key, &materials.digest).unwrap_err();
        assert!(error.to_string().contains("Pi native material snapshot changed at"), "{error}");
        assert_eq!(fs::read(home.join("models.json")).unwrap(), br#"{"providers":{}}"#);
    }
    fs::remove_dir_all(root).unwrap();
}
