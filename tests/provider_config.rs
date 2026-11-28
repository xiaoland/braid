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
fn provider_route_inputs_are_validated_at_cli_and_config_boundaries() {
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
    let diagnostic = stderr(&output);
    let error =
        diagnostic.lines().take_while(|line| !line.starts_with("Usage:")).collect::<Vec<_>>();
    for required in [
        "--llm-provider <ID>",
        "--llm-credential-kind <api_key|oauth|none>",
        "--llm-base-url <URL>",
    ] {
        assert!(error.iter().any(|line| line.trim() == required), "{required}: {diagnostic}");
    }

    let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&example)
        .output()
        .expect("check file-based example");
    assert!(output.status.success(), "{}", stderr(&output));

    let example_text = fs::read_to_string(&example).expect("read example");
    let mut coupled: toml::Value = toml::from_str(&example_text).expect("parse example");
    coupled["profiles"][0].as_table_mut().unwrap().insert("provider".into(), "openai".into());
    let coupled_path = std::env::temp_dir()
        .join(format!("braid-coupled-provider-config-{}.toml", std::process::id()));
    fs::write(&coupled_path, toml::to_string(&coupled).unwrap())
        .expect("write adapter-coupled config");
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

    coupled["schema_version"] = 2.into();
    let legacy_codex = toml::to_string(&coupled).unwrap();
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
    for (field, value, expected) in [
        ("api_key_environment", "PROVIDER_KEY", "unknown field `api_key_environment`"),
        ("api_key_file", "/absolute/path/to/key.toml", "uses legacy api_key_file"),
    ] {
        let path = std::env::temp_dir()
            .join(format!("braid-legacy-{field}-config-{}.toml", std::process::id()));
        let mut config: toml::Table = toml::from_str(&example_text).unwrap();
        let provider = toml::Table::from_iter([
            ("id".into(), "example-llm".into()),
            (field.into(), value.into()),
        ]);
        config.insert("llm_providers".into(), vec![toml::Value::Table(provider)].into());
        fs::write(&path, toml::to_string(&config).unwrap()).expect("write legacy config");
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
    let mut config: toml::Value = toml::from_str(example).expect("parse starter config");
    for runtime in config["runtimes"].as_array_mut().unwrap() {
        runtime["adapter_type"] = "pi".into();
        runtime["version"] = "1.0.2".into();
        runtime["home"] = directory.to_str().unwrap().into();
    }
    for profile in config["profiles"].as_array_mut().unwrap() {
        profile["adapter_type"] = "pi".into();
        profile["adapter_version"] = "1.0.2".into();
        profile.as_table_mut().unwrap().insert("provider".into(), "example-llm".into());
        profile["model"] = "example-chat".into();
    }
    let provider: toml::Value = toml::from_str(&format!(
        "id = \"example-llm\"\nbase_url = \"https://llm.example.invalid/v1\"\ncredential = {credential_fragment}\n[[models]]\nid = \"example-chat\"\n"
    )).unwrap();
    config
        .as_table_mut()
        .unwrap()
        .insert("llm_providers".into(), toml::Value::Array(vec![provider]));
    let path = directory.join("config.toml");
    fs::write(&path, toml::to_string(&config).unwrap()).expect("write Pi config");
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
fn profile_inspect_reports_the_selected_pi_route_and_credential_kind() {
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
        let report = stdout(&output);
        let kind = match name {
            "api-key" => "ApiKey",
            "oauth" => "OAuth",
            _ => "None",
        };
        assert!(report.contains(&format!("credential: {kind}")), "{report}");
        assert!(report.contains("provider: example-llm"), "{report}");
        assert!(report.contains("model: example-chat"), "{report}");
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

    for (name, expected) in [
        ("missing-provider", "requires provider for the Pi adapter"),
        ("missing-model", "requires an explicit model for the Pi adapter"),
        ("missing-base-url", "requires base_url for Pi models.json derivation"),
        ("undeclared-model", "is not declared in llm_providers"),
    ] {
        reject_config_case(
            &example,
            &temporary.join(name),
            |path| {
                let mut config: toml::Value =
                    toml::from_str(&fs::read_to_string(path).unwrap()).unwrap();
                match name {
                    "missing-provider" => {
                        config["profiles"][0].as_table_mut().unwrap().remove("provider");
                    }
                    "missing-model" => {
                        config["profiles"][0].as_table_mut().unwrap().remove("model");
                    }
                    "missing-base-url" => {
                        config["llm_providers"][0].as_table_mut().unwrap().remove("base_url");
                    }
                    _ => {
                        config["llm_providers"][0]["models"][0]["id"] = "other-model".into();
                    }
                }
                fs::write(path, toml::to_string(&config).unwrap()).unwrap();
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
    let parsed: toml::Value = toml::from_str(&generated).expect("init writes valid TOML");
    let profiles = parsed["profiles"].as_array().expect("generated profiles");
    assert!(!profiles.is_empty());
    for profile in profiles {
        assert_eq!(profile["provider"].as_str(), Some("example-llm"));
        assert_eq!(profile["model"].as_str(), Some("example-chat"));
    }
    assert_eq!(parsed["llm_providers"][0]["credential"]["kind"].as_str(), Some("none"));
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
    let access = "access\0token\n\"\\";
    let refresh = "refresh\u{0008}token";
    let account = "测试\u{0085}account";
    let api_key = "key\0\u{0008}\n\"\\";
    fs::create_dir_all(&temporary).expect("create temporary directory");
    fs::write(
        &pi_auth,
        serde_json::to_string_pretty(&serde_json::json!({
            "example-llm": {
                "type": "oauth",
                "access": access,
                "refresh": refresh,
                "expires": 4_102_444_800_000_u64,
                "account": account,
                "vendor_specific": "dropped-with-warning"
            },
            "other-provider": {"type": "api_key", "key": api_key}
        }))
        .expect("encode auth.json"),
    )
    .expect("write fake Pi auth.json");

    let import = |provider| {
        let mut command = braid();
        command
            .args(["provider", "import-auth", provider, "--from"])
            .arg(&pi_auth)
            .arg("--user-home")
            .arg(&user_home);
        command
    };
    let output = import("example-llm").output().expect("import OAuth credential");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(stderr(&output).contains("dropping provider-specific OAuth field \"vendor_specific\""));
    let imported = user_home.join("secrets/example-llm.toml");
    let text = fs::read_to_string(&imported).expect("read imported credential");
    let parsed: toml::Value = toml::from_str(&text).expect("import must write valid TOML");
    assert_eq!(parsed["access"].as_str(), Some(access));
    assert_eq!(parsed["refresh"].as_str(), Some(refresh));
    assert_eq!(parsed["expires"].as_integer(), Some(4_102_444_800_000));
    assert_eq!(parsed["account"].as_str(), Some(account));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(fs::metadata(&imported).expect("metadata").permissions().mode() & 0o777, 0o600);
    }

    let output = import("other-provider").output().expect("import API key credential");
    assert!(output.status.success(), "{}", stderr(&output));
    let imported_key = fs::read_to_string(user_home.join("secrets/other-provider.toml"))
        .expect("read imported api key");
    let parsed: toml::Value = toml::from_str(&imported_key).expect("API key is valid TOML");
    assert_eq!(parsed["provider_api_key"].as_str(), Some(api_key));

    let output = import("missing").output().expect("reject missing provider");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("has no credential for \"missing\""), "{}", stderr(&output));

    fs::write(&pi_auth, r#"{"example-llm":{"type":"api_key","key":"rotated"}}"#)
        .expect("rotate the source credential");
    let output = import("example-llm").output().expect("reject overwrite without force");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("already exists"), "{}", stderr(&output));
    assert_eq!(fs::read_to_string(&imported).unwrap(), text);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&imported, fs::Permissions::from_mode(0o644))
            .expect("simulate an existing unprotected credential");
        let output =
            import("example-llm").arg("--force").output().expect("replace an existing credential");
        assert!(output.status.success(), "{}", stderr(&output));
        assert_eq!(fs::metadata(&imported).expect("metadata").permissions().mode() & 0o777, 0o600);
        let rotated: toml::Value = toml::from_str(&fs::read_to_string(&imported).unwrap()).unwrap();
        assert_eq!(rotated["provider_api_key"].as_str(), Some("rotated"));
    }

    // BRAID_USER_HOME is honored without an explicit --user-home flag.
    let env_home = temporary.join("env-home");
    let output = braid()
        .args(["provider", "import-auth", "example-llm", "--from"])
        .arg(&pi_auth)
        .env("BRAID_USER_HOME", &env_home)
        .output()
        .expect("import honors BRAID_USER_HOME");
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(env_home.join("secrets/example-llm.toml").is_file());
    let _ = fs::remove_dir_all(&temporary);
}

#[test]
fn provider_import_rejects_ids_that_escape_the_secrets_directory() {
    let temporary =
        std::env::temp_dir().join(format!("braid-import-path-{}", uuid::Uuid::now_v7()));
    let user_home = temporary.join("home");
    fs::create_dir_all(&temporary).unwrap();
    let source = temporary.join("auth.json");
    fs::write(&source, r#"{"../escaped":{"type":"api_key","key":"synthetic-key"}}"#).unwrap();
    let output = braid()
        .args(["provider", "import-auth", "../escaped", "--from"])
        .arg(&source)
        .arg("--user-home")
        .arg(&user_home)
        .output()
        .unwrap();
    let escaped = user_home.join("escaped.toml").exists();
    fs::remove_dir_all(&temporary).unwrap();
    assert!(!output.status.success(), "a provider id must not become a path");
    assert!(
        stderr(&output).contains("provider id must be a single file name"),
        "{}",
        stderr(&output)
    );
    assert!(!escaped, "credential escaped the secrets directory");
}
