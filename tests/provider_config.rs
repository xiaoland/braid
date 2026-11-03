use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn braid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_braid"))
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn provider_credentials_are_configured_only_through_secret_files() {
    let output = braid().args(["setup", "--help"]).output().expect("run setup help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert!(help.contains("--adapter <pi|codex>"), "{help}");
    assert!(help.contains("--llm-provider <ID>"), "{help}");
    assert!(help.contains("--llm-protocol <PROTOCOL>"), "{help}");
    assert!(help.contains("--api-key-file <PATH>"), "{help}");
    assert!(!help.contains("--api-key-environment"));
    assert!(!help.contains("deepseek-chat"));

    let output = braid().args(["init", "--help"]).output().expect("run init help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert!(help.contains("--llm-provider <ID>"), "{help}");
    assert!(help.contains("--llm-protocol <PROTOCOL>"), "{help}");
    assert!(help.contains("--api-key-file <PATH>"), "{help}");
    assert!(!help.contains("--api-key-environment"));
    assert!(!help.contains("deepseek-chat"));

    let output = braid()
        .args([
            "init",
            "--source",
            "/repository",
            "--config",
            "/harness/config.toml",
            "--name",
            "demo",
            "--adapter",
            "pi",
            "--executable",
            "/pi",
            "--model",
            "example-chat",
        ])
        .output()
        .expect("run init without secret file");
    assert!(!output.status.success());
    for required in ["--llm-provider <ID>", "--llm-protocol <PROTOCOL>", "--api-key-file <PATH>"] {
        assert!(stderr(&output).contains(required), "{required}: {}", stderr(&output));
    }

    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&example)
        .output()
        .expect("check file-based example");
    assert!(output.status.success(), "{}", stderr(&output));

    let example_text = fs::read_to_string(&example).expect("read example");
    let legacy = example_text.replacen(
        "[tools]",
        "[[llm_providers]]\n\nid = \"example-llm\"\n\nprotocol = \"openai-compatible\"\n\napi_key_environment = \"DEEPSEEK_API_KEY\"\n\n[tools]",
        1,
    );
    let legacy_path = std::env::temp_dir()
        .join(format!("braid-legacy-provider-config-{}.toml", std::process::id()));
    fs::write(&legacy_path, legacy).expect("write legacy config");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&legacy_path)
        .output()
        .expect("reject environment-based provider config");
    let _ = fs::remove_file(&legacy_path);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown field `api_key_environment`"), "{}", stderr(&output));

    let coupled = example_text.replacen(
        "adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\nmodel = \"gpt-5.6-sol\"",
        "adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\n\nprovider = \"openai\"\n\nmodel = \"gpt-5.6-sol\"",
        1,
    );
    let coupled_path = std::env::temp_dir()
        .join(format!("braid-coupled-provider-config-{}.toml", std::process::id()));
    fs::write(&coupled_path, &coupled).expect("write adapter-coupled config");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&coupled_path)
        .output()
        .expect("reject Codex provider reference");
    let _ = fs::remove_file(&coupled_path);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("provider is supported only by the Pi adapter"),
        "{}",
        stderr(&output)
    );

    let legacy_codex = coupled.replacen("schema_version = 3", "schema_version = 2", 1);
    let legacy_codex_path = std::env::temp_dir()
        .join(format!("braid-legacy-codex-provider-config-{}.toml", std::process::id()));
    fs::write(&legacy_codex_path, legacy_codex).expect("write legacy Codex config");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&legacy_codex_path)
        .output()
        .expect("read legacy Codex provider label as inert");
    let _ = fs::remove_file(&legacy_codex_path);
    assert!(output.status.success(), "{}", stderr(&output));
}

fn write_pi_config(example: &str, directory: &Path, api_key_reference: &str) -> PathBuf {
    fs::create_dir_all(directory).expect("create Pi material directory");
    let models = serde_json::json!({
        "providers": {
            "example-llm": {
                "baseUrl": "https://llm.example.invalid/v1",
                "apiKey": api_key_reference,
                "models": [{"id": "example-chat"}]
            }
        }
    });
    fs::write(directory.join("models.json"), serde_json::to_vec(&models).expect("encode models"))
        .expect("write models");

    let config = example
        .replace("adapter_type = \"codex\"", "adapter_type = \"pi\"")
        .replace("version = \"codex-cli 0.147.0-alpha.6.5\"", "version = \"1.0.2\"")
        .replace("adapter_version = \"codex-cli 0.147.0-alpha.6.5\"", "adapter_version = \"1.0.2\"")
        .replace("model = \"gpt-5.6-sol\"", "model = \"example-chat\"")
        .replace(
            "adapter_version = \"1.0.2\"\nmodel = \"example-chat\"",
            "adapter_version = \"1.0.2\"\nprovider = \"example-llm\"\nmodel = \"example-chat\"",
        )
        .replacen(
            "home = \"/absolute/path/to/instance/provider/codex\"",
            &format!("home = \"{}\"", directory.display()),
            1,
        )
        .replacen(
            "[tools]",
            "[[llm_providers]]\nid = \"example-llm\"\nprotocol = \"openai-compatible\"\napi_key_file = \"/absolute/path/to/user/secrets/example-llm.toml\"\n\n[[llm_providers.models]]\nmodel_id = \"example-chat\"\n\n[tools]",
            1,
        );
    let path = directory.join("config.toml");
    fs::write(&path, config).expect("write Pi config");
    path
}

#[test]
fn pi_materials_use_only_the_managed_credential_reference() {
    let example =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .expect("read example");
    let temporary =
        std::env::temp_dir().join(format!("braid-pi-provider-materials-{}", std::process::id()));

    for (index, reference) in
        ["$BRAID_PROVIDER_API_KEY", "${BRAID_PROVIDER_API_KEY}"].into_iter().enumerate()
    {
        let directory = temporary.join(format!("managed-{index}"));
        let config = write_pi_config(&example, &directory, reference);
        let output = braid()
            .args(["profile", "inspect", "--config"])
            .arg(&config)
            .args(["--profile", "issue-codex"])
            .output()
            .expect("inspect managed Pi credential reference");
        assert!(output.status.success(), "{reference}: {}", stderr(&output));
    }

    let directory = temporary.join("missing-provider");
    let config = write_pi_config(&example, &directory, "$BRAID_PROVIDER_API_KEY");
    let missing_provider = fs::read_to_string(&config).expect("read Pi config").replacen(
        "provider = \"example-llm\"\n",
        "",
        1,
    );
    fs::write(&config, missing_provider).expect("remove Pi provider");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&config)
        .output()
        .expect("reject Pi profile without provider");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("requires provider for the Pi adapter"),
        "{}",
        stderr(&output)
    );

    let directory = temporary.join("legacy");
    let config = write_pi_config(&example, &directory, "$DEEPSEEK_API_KEY");
    let output = braid()
        .args(["profile", "inspect", "--config"])
        .arg(&config)
        .args(["--profile", "issue-codex"])
        .output()
        .expect("reject legacy Pi credential reference");
    let _ = fs::remove_dir_all(&temporary);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unsupported or conflicting key \"apiKey\""),
        "{}",
        stderr(&output)
    );
}
