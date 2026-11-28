use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn braid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_braid"))
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn multi_repo_config(example: &str, temporary: &Path) -> toml::Value {
    let mut config: toml::Value = toml::from_str(example).expect("parse starter config");
    config["github"].as_table_mut().unwrap().insert(
        "additional_repositories".into(),
        toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([(
            "name".into(),
            "owner/other".into(),
        )]))]),
    );
    let profiles = config["profiles"].as_array_mut().unwrap();
    for profile in &mut *profiles {
        let profile = profile.as_table_mut().unwrap();
        profile.insert("repository".into(), "owner/repository".into());
        profile
            .insert("workspace".into(), temporary.join("primary-source").to_str().unwrap().into());
    }
    let other_profiles: Vec<_> = profiles
        .iter()
        .map(|profile| {
            let mut other = profile.clone();
            other["id"] = format!("{}-other", profile["id"].as_str().unwrap()).into();
            other["repository"] = "owner/other".into();
            other["workspace"] = temporary.join("other-source").to_str().unwrap().into();
            other
        })
        .collect();
    profiles.extend(other_profiles);
    config
}

#[test]
fn multi_repository_config_requires_repo_scoped_profiles() {
    let example =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("config.example.toml"))
            .expect("read config example");
    let temporary =
        std::env::temp_dir().join(format!("braid-multi-repo-config-{}", std::process::id()));
    fs::create_dir_all(&temporary).expect("create temporary directory");
    let path = temporary.join("config.toml");
    fs::write(&path, toml::to_string(&multi_repo_config(&example, &temporary)).unwrap())
        .expect("write multi-repo config");

    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&path)
        .output()
        .expect("check multi-repository config");
    assert!(output.status.success(), "{}", stderr(&output));

    let output = braid()
        .args(["profile", "inspect", "--config"])
        .arg(&path)
        .args(["--profile", "issue-codex-other", "--json"])
        .output()
        .expect("inspect repo-scoped profile");
    assert!(output.status.success(), "{}", stderr(&output));

    let profile: serde_json::Value = serde_json::from_slice(&output.stdout).expect("profile JSON");
    assert_eq!(profile["id"], "issue-codex-other");
    assert_eq!(profile["repository"], "owner/other");
    assert_eq!(profile["workspace"], temporary.join("other-source").to_str().unwrap());

    for (case, expected) in [
        ("omitted", "requires an explicit repository"),
        ("unconfigured", "unconfigured GitHub repository"),
        ("workspace", "must share one source checkout"),
    ] {
        let mut candidate = multi_repo_config(&example, &temporary);
        let profile = candidate["profiles"][0].as_table_mut().unwrap();
        match case {
            "omitted" => {
                profile.remove("repository");
            }
            "unconfigured" => {
                profile.insert("repository".into(), "owner/unconfigured".into());
            }
            _ => {
                profile.insert(
                    "workspace".into(),
                    temporary.join("different-source").to_str().unwrap().into(),
                );
            }
        }
        fs::write(&path, toml::to_string(&candidate).unwrap()).expect("write invalid config");
        let output = braid().args(["config", "check", "--config"]).arg(&path).output().unwrap();
        assert!(!output.status.success(), "{case}");
        assert!(stderr(&output).contains(expected), "{case}: {}", stderr(&output));
    }
    fs::remove_dir_all(&temporary).unwrap();
}

#[test]
fn instance_selectors_route_to_the_expected_config_and_doctor_names_cross_instance_conflicts() {
    let home = std::env::temp_dir().join(format!("braid-instances-{}", uuid::Uuid::now_v7()));
    let mut entries = Vec::new();
    for (key, app_id, schema) in [("first", 100, 3), ("second", 200, 2)] {
        let directory = home.join(key);
        fs::create_dir_all(&directory).unwrap();
        let mut config: toml::Value =
            toml::from_str(include_str!("../config.example.toml")).unwrap();
        config["schema_version"] = schema.into();
        config["instance"]["key"] = key.into();
        config["github"]["app_id"] = app_id.into();
        config["github"]["private_key_file"] =
            directory.join("missing.pem").to_str().unwrap().into();
        config["runtimes"][0]["executable"] = "/usr/bin/false".into();
        config["tools"]["gh"] = "/usr/bin/false".into();
        config["tools"]["wrangler"] = "/usr/bin/false".into();
        fs::write(directory.join("config.toml"), toml::to_string(&config).unwrap()).unwrap();
        entries.push(format!(
            "[[instances]]\nkey='{key}'\nhome='{key}'\ngithub_app_id={app_id}\nrepository='owner/repository'\n"
        ));
    }
    fs::write(
        home.join("registry.toml"),
        format!("schema_version=1\ndefault_instance='first'\n{}", entries.join("\n")),
    )
    .unwrap();
    let second_dir = home.join("second");
    let first_config = home.join("first/config.toml");
    let command = || {
        let mut command = braid();
        command.env_clear().env("PATH", "/usr/bin:/bin").env("BRAID_USER_HOME", &home);
        command
    };
    for (args, instance, override_home, expected) in [
        (vec![], None, None, "first"),
        (vec![], Some("second"), None, "second"),
        (vec!["--instance", "second"], Some("first"), None, "second"),
        (vec!["--instance", "first"], None, Some(&second_dir), "second"),
        (
            vec!["--config", first_config.to_str().unwrap()],
            Some("second"),
            Some(&second_dir),
            "first",
        ),
    ] {
        let mut command = command();
        if let Some(value) = instance {
            command.env("BRAID_INSTANCE", value);
        }
        if let Some(value) = override_home {
            command.env("BRAID_INSTANCE_HOME", value);
        }
        let output = command.args(["config", "check", "--json"]).args(args).output().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(summary["instance_key"], expected);
        assert_eq!(
            summary["database"],
            home.join(expected).join("state/braid.sqlite3").to_str().unwrap()
        );
    }
    let output = command().args(["migrate", "apply", "--instance", "second"]).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let output = command().args(["status", "--instance", "second", "--json"]).output().unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["config_schema"], 2);
    assert_eq!(status["database"]["pending_migrations"], 0);
    assert!(status["database"]["schema_version"].as_u64().unwrap() > 0);
    let output = command().args(["doctor", "--json"]).output().unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ports = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "instance ports")
        .unwrap();
    assert_eq!(ports["state"], "fail");
    let detail = ports["detail"].as_str().unwrap();
    assert!(detail.contains("8080 on second ingress"), "{detail}");
    assert!(detail.contains("8081 on second health"), "{detail}");
    fs::remove_dir_all(home).unwrap();
}
