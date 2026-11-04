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
    assert!(help.contains("--reasoning <LEVEL>"), "{help}");
    assert!(help.contains("--api-key-file <PATH>"), "{help}");
    assert!(!help.contains("--llm-protocol"));
    assert!(!help.contains("--api-key-environment"));
    assert!(!help.contains("deepseek-chat"));

    let output = braid().args(["init", "--help"]).output().expect("run init help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert!(help.contains("--llm-provider <ID>"), "{help}");
    assert!(help.contains("--reasoning <LEVEL>"), "{help}");
    assert!(help.contains("--api-key-file <PATH>"), "{help}");
    assert!(!help.contains("--llm-protocol"));
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
    let required = "--llm-provider <ID>";
    assert!(stderr(&output).contains(required), "{required}: {}", stderr(&output));

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
        "[[llm_providers]]\n\nid = \"example-llm\"\n\napi_key_environment = \"DEEPSEEK_API_KEY\"\n\n[tools]",
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
        "adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\nmodel = \"example-model\"",
        "adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\n\nprovider = \"openai\"\n\nmodel = \"example-model\"",
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
        .replace("model = \"example-model\"", "model = \"example-chat\"")
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
            "[[llm_providers]]\nid = \"example-llm\"\napi_key_file = \"/absolute/path/to/user/secrets/example-llm.toml\"\n\n[tools]",
            1,
        );
    let path = directory.join("config.toml");
    fs::write(&path, config).expect("write Pi config");
    path
}

fn inspect_profile(config: &Path) -> std::process::Output {
    braid()
        .args(["profile", "inspect", "--config"])
        .arg(config)
        .args(["--profile", "issue-codex"])
        .output()
        .expect("inspect Pi profile")
}

fn remove_secret_reference(config: &Path) {
    let without_secret = fs::read_to_string(config)
        .expect("read Pi config")
        .lines()
        .filter(|line| !line.starts_with("api_key_file ="))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(config, format!("{without_secret}\n")).expect("remove Pi secret reference");
}

fn remove_model_api_key(directory: &Path) {
    let models_path = directory.join("models.json");
    let mut models: serde_json::Value =
        serde_json::from_slice(&fs::read(&models_path).expect("read models"))
            .expect("parse models");
    models["providers"]["example-llm"].as_object_mut().expect("provider object").remove("apiKey");
    fs::write(&models_path, serde_json::to_vec(&models).expect("encode models"))
        .expect("write keyless models");
}

#[test]
fn init_generates_an_explicit_keyless_pi_route() {
    let temporary = std::env::temp_dir().join(format!("braid-init-keyless-{}", std::process::id()));
    let source = temporary.join("source");
    let harness = temporary.join("harness");
    fs::create_dir_all(&source).expect("create source");
    fs::create_dir_all(&harness).expect("create harness");
    fs::write(source.join("README.md"), "demo\n").expect("write source file");
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "README.md"],
        vec![
            "-c",
            "user.email=braid@example.invalid",
            "-c",
            "user.name=Braid Test",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    ] {
        let output =
            Command::new("git").arg("-C").arg(&source).args(args).output().expect("run git");
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let executable = temporary.join("fake-pi");
    fs::write(&executable, "#!/bin/sh\necho 'pi 1.2.3'\n").expect("write fake Pi");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(&executable).expect("fake Pi metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake Pi executable");
    }

    let config = harness.join("config.toml");
    let output = braid()
        .args(["init", "--source"])
        .arg(&source)
        .args(["--config"])
        .arg(&config)
        .args(["--name", "demo", "--adapter", "pi", "--executable"])
        .arg(&executable)
        .args(["--model", "example-chat", "--llm-provider", "example-llm"])
        .output()
        .expect("run keyless Pi init");
    assert!(output.status.success(), "{}", stderr(&output));
    let generated = fs::read_to_string(&config).expect("read generated config");
    assert!(generated.contains("provider = \"example-llm\""), "{generated}");
    assert!(generated.contains("model = \"example-chat\""), "{generated}");
    assert!(!generated.contains("api_key_file"), "{generated}");
    assert!(!generated.contains("protocol"), "{generated}");
    assert!(!generated.contains("allowances"), "{generated}");
    fs::write(&config, generated.replace("http://127.0.0.1:4318", "http://127.0.0.1:9"))
        .expect("make OTLP endpoint unavailable");
    fs::create_dir_all(harness.join("state/backups")).expect("create runtime directories");
    let isolated_home = temporary.join("user-home");

    let output = braid()
        .args(["doctor", "--config"])
        .arg(&config)
        .env("BRAID_USER_HOME", &isolated_home)
        .output()
        .expect("run doctor with unavailable optional OTLP");
    let checks = stdout(&output);
    assert!(checks.contains("Unavailable\tPi provider"), "{checks}");
    assert!(checks.contains("Unavailable\tOTLP endpoint"), "{checks}");
    let _ = fs::remove_dir_all(&temporary);
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
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
        let output = inspect_profile(&write_pi_config(&example, &directory, reference));
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

    let directory = temporary.join("missing-model");
    let config = write_pi_config(&example, &directory, "$BRAID_PROVIDER_API_KEY");
    let missing_model = fs::read_to_string(&config).expect("read Pi config").replacen(
        "model = \"example-chat\"\n",
        "",
        1,
    );
    fs::write(&config, missing_model).expect("remove Pi model");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&config)
        .output()
        .expect("reject Pi profile without model");
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("requires an explicit model for the Pi adapter"),
        "{}",
        stderr(&output)
    );

    let directory = temporary.join("keyless");
    let config = write_pi_config(&example, &directory, "$BRAID_PROVIDER_API_KEY");
    remove_secret_reference(&config);
    remove_model_api_key(&directory);
    let output = inspect_profile(&config);
    assert!(output.status.success(), "{}", stderr(&output));

    let directory = temporary.join("lost-native-auth");
    let config = write_pi_config(&example, &directory, "$BRAID_PROVIDER_API_KEY");
    remove_secret_reference(&config);
    remove_model_api_key(&directory);
    fs::write(directory.join("auth.json"), "{}").expect("write runtime-home auth");
    let output = inspect_profile(&config);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("auth.json, but Profile material snapshots do not carry it"),
        "{}",
        stderr(&output)
    );

    let directory = temporary.join("missing-secret");
    let config = write_pi_config(&example, &directory, "$BRAID_PROVIDER_API_KEY");
    remove_secret_reference(&config);
    let output = inspect_profile(&config);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unsupported or conflicting key \"apiKey\""),
        "{}",
        stderr(&output)
    );

    let directory = temporary.join("legacy");
    let output = inspect_profile(&write_pi_config(&example, &directory, "$DEEPSEEK_API_KEY"));
    let _ = fs::remove_dir_all(&temporary);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("unsupported or conflicting key \"apiKey\""),
        "{}",
        stderr(&output)
    );
}
