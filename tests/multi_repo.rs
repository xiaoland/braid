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
    config["github"]["additional_repositories"] = toml::Value::Array(vec![toml::Value::Table(
        toml::Table::from_iter([("name".into(), "owner/other".into())]),
    )]);
    let profiles = config["profiles"].as_array_mut().unwrap();
    for profile in &mut *profiles {
        profile["repository"] = "owner/repository".into();
        profile["workspace"] = temporary.join("primary-source").to_str().unwrap().into();
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
