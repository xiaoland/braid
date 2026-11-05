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
    assert!(help.contains("--llm-credential-kind <api_key|oauth|none>"), "{help}");
    assert!(help.contains("--llm-credential-file <PATH>"), "{help}");
    assert!(!help.contains("--api-key-file"));
    assert!(!help.contains("--api-key-environment"));
    assert!(!help.contains("deepseek-chat"));

    let output = braid().args(["init", "--help"]).output().expect("run init help");
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    assert!(help.contains("--llm-provider <ID>"), "{help}");
    assert!(help.contains("--reasoning <LEVEL>"), "{help}");
    assert!(help.contains("--llm-credential-kind <api_key|oauth|none>"), "{help}");
    assert!(help.contains("--llm-credential-file <PATH>"), "{help}");
    assert!(!help.contains("--api-key-file"));
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
        .expect("run init without provider and credential kind");
    assert!(!output.status.success());
    for required in ["--llm-provider <ID>", "--llm-credential-kind <api_key|oauth|none>"] {
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

#[test]
fn schema3_rejects_legacy_provider_surfaces() {
    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
    let example_text = fs::read_to_string(&example).expect("read example");
    for (name, fragment, expected) in [
        (
            "environment",
            "[[llm_providers]]\n\nid = \"example-llm\"\n\napi_key_environment = \"DEEPSEEK_API_KEY\"\n\n[tools]",
            "unknown field `api_key_environment`",
        ),
        (
            "api_key_file",
            "[[llm_providers]]\nid = \"example-llm\"\napi_key_file = \"/absolute/path/to/user/secrets/example-llm.toml\"\n\n[tools]",
            "uses legacy api_key_file",
        ),
    ] {
        let path = std::env::temp_dir()
            .join(format!("braid-legacy-{name}-config-{}.toml", std::process::id()));
        fs::write(&path, example_text.replacen("[tools]", fragment, 1))
            .expect("write legacy config");
        let output = braid()
            .args(["config", "check", "--config"])
            .arg(&path)
            .output()
            .expect("reject legacy provider config");
        let _ = fs::remove_file(&path);
        assert!(!output.status.success());
        assert!(stderr(&output).contains(expected), "{expected}: {}", stderr(&output));
    }
}

/// Write a Pi-flavored variant of the example config. models.json is derived
/// from the `llm_providers` entry, so no template file is needed; `directory`
/// stands in for the Pi runtime home and may carry extra files for negative
/// cases.
fn write_pi_config(example: &str, directory: &Path, credential_fragment: &str) -> PathBuf {
    fs::create_dir_all(directory).expect("create Pi material directory");
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
            &format!(
                "[[llm_providers]]\nid = \"example-llm\"\nbase_url = \"https://llm.example.invalid/v1\"\ncredential = {credential_fragment}\n\n[[llm_providers.models]]\nid = \"example-chat\"\n\n[tools]"
            ),
            1,
        );
    let path = directory.join("config.toml");
    fs::write(&path, config).expect("write Pi config");
    path
}

fn api_key_credential(directory: &Path) -> String {
    fs::create_dir_all(directory).expect("create credential directory");
    let secret = directory.join("secret.toml");
    fs::write(&secret, "provider_api_key = \"test-key\"\n").expect("write api key secret");
    format!("{{ kind = \"api_key\", file = \"{}\" }}", secret.display())
}

fn oauth_credential(directory: &Path) -> String {
    fs::create_dir_all(directory).expect("create credential directory");
    let secret = directory.join("oauth.toml");
    fs::write(
        &secret,
        "access = \"test-access\"\nrefresh = \"test-refresh\"\nexpires = 4102444800000\naccount = \"tester\"\n",
    )
    .expect("write oauth credential");
    format!("{{ kind = \"oauth\", file = \"{}\" }}", secret.display())
}

fn inspect_profile(config: &Path) -> std::process::Output {
    braid()
        .args(["profile", "inspect", "--config"])
        .arg(config)
        .args(["--profile", "issue-codex"])
        .output()
        .expect("inspect Pi profile")
}

fn check_config(config: &Path) -> std::process::Output {
    braid().args(["config", "check", "--config"]).arg(config).output().expect("run config check")
}

fn reject_config_case(example: &str, directory: &Path, mutate: impl Fn(&Path), expected: &str) {
    let credential = api_key_credential(directory);
    let config = write_pi_config(example, directory, &credential);
    mutate(&config);
    let output = check_config(&config);
    assert!(
        !output.status.success(),
        "case {expected} unexpectedly succeeded:\nstdout: {}\nstderr: {}",
        stdout(&output),
        stderr(&output)
    );
    assert!(stderr(&output).contains(expected), "{expected}: {}", stderr(&output));
}

#[test]
fn pi_materials_are_derived_from_provider_declarations() {
    let example =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .expect("read example");
    let temporary =
        std::env::temp_dir().join(format!("braid-pi-provider-materials-{}", std::process::id()));

    for (name, credential_fragment) in [
        ("api-key", api_key_credential(&temporary.join("api-key"))),
        ("oauth", oauth_credential(&temporary.join("oauth"))),
        ("none", "{ kind = \"none\" }".to_owned()),
    ] {
        let directory = temporary.join(name);
        let output = inspect_profile(&write_pi_config(&example, &directory, &credential_fragment));
        assert!(output.status.success(), "{name}: {}", stderr(&output));
    }
    let _ = fs::remove_dir_all(&temporary);
}

#[test]
fn pi_derivation_rejects_bad_provider_surfaces() {
    let example =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .expect("read example");
    let temporary =
        std::env::temp_dir().join(format!("braid-pi-derivation-rejects-{}", std::process::id()));

    for (name, needle, replacement, expected) in [
        (
            "missing-provider",
            "\nprovider = \"example-llm\"",
            "",
            "requires provider for the Pi adapter",
        ),
        (
            "missing-model",
            "\nmodel = \"example-chat\"",
            "",
            "requires an explicit model for the Pi adapter",
        ),
        (
            "missing-base-url",
            "\nbase_url = \"https://llm.example.invalid/v1\"",
            "",
            "requires base_url for Pi models.json derivation",
        ),
        (
            "undeclared-model",
            "\nid = \"example-chat\"",
            "\nid = \"other-model\"",
            "is not declared in llm_providers",
        ),
    ] {
        reject_config_case(
            &example,
            &temporary.join(name),
            |config| {
                let mutated = fs::read_to_string(config).expect("read Pi config").replacen(
                    needle,
                    replacement,
                    1,
                );
                fs::write(config, mutated).expect("mutate config");
            },
            expected,
        );
    }

    reject_config_case(
        &example,
        &temporary.join("template-models"),
        |config| {
            fs::write(config.parent().expect("config parent").join("models.json"), "{}")
                .expect("write template models.json");
        },
        "models.json is derived from llm_providers metadata",
    );

    // Schema 2 keeps the legacy models.json template path readable.
    let directory = temporary.join("legacy-schema2");
    fs::create_dir_all(&directory).expect("create schema-2 directory");
    fs::write(
        directory.join("models.json"),
        serde_json::to_vec(&serde_json::json!({
            "providers": {
                "example-llm": {
                    "baseUrl": "https://llm.example.invalid/v1",
                    "apiKey": "$BRAID_PROVIDER_API_KEY",
                    "models": [{"id": "example-chat"}]
                }
            }
        }))
        .expect("encode models"),
    )
    .expect("write schema-2 models.json");
    let credential = api_key_credential(&directory);
    let config = write_pi_config(&example, &directory, &credential);
    let schema2 = fs::read_to_string(&config).expect("read generated config").replacen(
        "schema_version = 3",
        "schema_version = 2",
        1,
    );
    fs::write(&config, schema2).expect("write schema-2 config");
    let output = inspect_profile(&config);
    let _ = fs::remove_dir_all(&temporary);
    assert!(output.status.success(), "{}", stderr(&output));
}

#[test]
fn runtime_home_auth_json_is_always_rejected_with_import_hint() {
    let example =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .expect("read example");
    let temporary = std::env::temp_dir().join(format!("braid-pi-home-auth-{}", std::process::id()));
    let config = write_pi_config(&example, &temporary, "{ kind = \"none\" }");
    fs::write(temporary.join("auth.json"), "{}").expect("write runtime-home auth");
    let output = inspect_profile(&config);
    let _ = fs::remove_dir_all(&temporary);
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("auth.json, which Profile material snapshots never carry"),
        "{}",
        stderr(&output)
    );
    assert!(stderr(&output).contains("braid provider import-auth"), "{}", stderr(&output));
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
        .args([
            "--model",
            "example-chat",
            "--llm-provider",
            "example-llm",
            "--llm-credential-kind",
            "none",
            "--llm-base-url",
            "https://llm.example.invalid/v1",
        ])
        .output()
        .expect("run keyless Pi init");
    assert!(output.status.success(), "{}", stderr(&output));
    let generated = fs::read_to_string(&config).expect("read generated config");
    assert!(generated.contains("provider = \"example-llm\""), "{generated}");
    assert!(generated.contains("model = \"example-chat\""), "{generated}");
    assert!(generated.contains("kind = \"none\""), "{generated}");
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
    assert!(checks.contains("Pass\tPi provider"), "{checks}");
    assert!(checks.contains("explicit keyless route"), "{checks}");
    assert!(checks.contains("Unavailable\tOTLP endpoint"), "{checks}");
    let _ = fs::remove_dir_all(&temporary);
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
}

#[test]
fn provider_import_auth_converts_pi_credentials() {
    let temporary =
        std::env::temp_dir().join(format!("braid-provider-import-auth-{}", std::process::id()));
    let user_home = temporary.join("user-home");
    let pi_auth = temporary.join("auth.json");
    fs::create_dir_all(&temporary).expect("create temporary directory");
    fs::write(
        &pi_auth,
        serde_json::to_string_pretty(&serde_json::json!({
            "example-llm": {
                "type": "oauth",
                "access": "access-token",
                "refresh": "refresh-token",
                "expires": 4_102_444_800_000_u64,
                "account": "tester@example.com",
                "vendor_specific": "dropped-with-warning"
            },
            "other-provider": {"type": "api_key", "key": "other-key"}
        }))
        .expect("encode auth.json"),
    )
    .expect("write fake Pi auth.json");

    let output = braid()
        .args(["provider", "import-auth", "example-llm", "--from"])
        .arg(&pi_auth)
        .args(["--user-home"])
        .arg(&user_home)
        .output()
        .expect("import OAuth credential");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(stderr(&output).contains("dropping provider-specific OAuth field \"vendor_specific\""));
    let imported = user_home.join("secrets/example-llm.toml");
    let text = fs::read_to_string(&imported).expect("read imported credential");
    assert!(text.contains("access = \"access-token\""), "{text}");
    assert!(text.contains("refresh = \"refresh-token\""), "{text}");
    assert!(text.contains("expires = 4102444800000"), "{text}");
    assert!(text.contains("account = \"tester@example.com\""), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs::metadata(&imported).expect("metadata").permissions().mode() & 0o777, 0o600);
    }

    let output = braid()
        .args(["provider", "import-auth", "other-provider", "--from"])
        .arg(&pi_auth)
        .args(["--user-home"])
        .arg(&user_home)
        .output()
        .expect("import API key credential");
    assert!(output.status.success(), "{}", stderr(&output));
    let imported_key = fs::read_to_string(user_home.join("secrets/other-provider.toml"))
        .expect("read imported api key");
    assert_eq!(imported_key, "provider_api_key = \"other-key\"\n");

    let output = braid()
        .args(["provider", "import-auth", "missing", "--from"])
        .arg(&pi_auth)
        .args(["--user-home"])
        .arg(&user_home)
        .output()
        .expect("reject missing provider");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("has no credential for \"missing\""), "{}", stderr(&output));

    let output = braid()
        .args(["provider", "import-auth", "example-llm", "--from"])
        .arg(&pi_auth)
        .args(["--user-home"])
        .arg(&user_home)
        .output()
        .expect("reject overwrite without force");
    let _ = fs::remove_dir_all(&temporary);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("already exists"), "{}", stderr(&output));
}
