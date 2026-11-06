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

fn multi_repo_config(example: &str, temporary: &Path) -> String {
    let other_workspace = temporary.join("other-source");
    let primary_workspace = temporary.join("primary-source");
    let config = example
        .replacen(
            "repository = \"owner/repository\"",
            "repository = \"owner/repository\"\nadditional_repositories = [{ name = \"owner/other\" }]",
            1,
        )
        .replace(
            "adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\nmodel = \"example-model\"",
            &format!(
                "adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\nrepository = \"owner/repository\"\nworkspace = \"{}\"\nmodel = \"example-model\"",
                primary_workspace.display()
            ),
        );
    config.replacen(
        "[profile_selection]",
        &format!(
            "[[profiles]]\n\
                 id = \"issue-other\"\n\
                 display_name = \"Other issue agent\"\n\
                 tags = [\"issue\"]\n\
                 adapter_type = \"codex\"\n\
                 adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\n\
                 repository = \"owner/other\"\n\
                 model = \"example-model\"\n\
                 user_instructions = \"Serve the other repository.\"\n\
                 workspace = \"{}\"\n\
                 github_actor_node_id = \"MDQ6VXNlcjEyMzQ1\"\n\
                 status_surfaces = [\"issue\"]\n\
                 github_context_soft_ratio = 0.8\n\
                 github_context_hard_bytes = 100000\n\
                 \n\
                 [[profiles]]\n\
                 id = \"pr-other\"\n\
                 display_name = \"Other implementation agent\"\n\
                 tags = [\"pr\", \"implementation\"]\n\
                 adapter_type = \"codex\"\n\
                 adapter_version = \"codex-cli 0.147.0-alpha.6.5\"\n\
                 repository = \"owner/other\"\n\
                 model = \"example-model\"\n\
                 user_instructions = \"Implement in the other repository.\"\n\
                 workspace = \"{}\"\n\
                 github_actor_node_id = \"MDQ6VXNlcjEyMzQ1\"\n\
                 status_surfaces = [\"pr\"]\n\
                 github_context_soft_ratio = 0.8\n\
                 github_context_hard_bytes = 100000\n\
                 \n\
                 [profile_selection]",
            other_workspace.display(),
            other_workspace.display()
        ),
        1,
    )
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
    fs::write(&path, multi_repo_config(&example, &temporary)).expect("write multi-repo config");

    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&path)
        .output()
        .expect("check multi-repository config");
    assert!(output.status.success(), "{}", stderr(&output));

    let output = braid()
        .args(["profile", "inspect", "--config"])
        .arg(&path)
        .args(["--profile", "issue-other"])
        .output()
        .expect("inspect repo-scoped profile");
    assert!(output.status.success(), "{}", stderr(&output));

    let missing_binding = fs::read_to_string(&path).expect("read multi config").replacen(
        "repository = \"owner/repository\"\nworkspace",
        "workspace",
        1,
    );
    fs::write(&path, missing_binding).expect("remove one repository binding");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&path)
        .output()
        .expect("reject omitted repository binding");
    assert!(!output.status.success());
    assert!(stderr(&output).contains("requires an explicit repository"), "{}", stderr(&output));

    let unconfigured = multi_repo_config(&example, &temporary).replacen(
        "repository = \"owner/other\"",
        "repository = \"owner/unconfigured\"",
        1,
    );
    fs::write(&path, unconfigured).expect("write unconfigured repository binding");
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&path)
        .output()
        .expect("reject unconfigured repository binding");
    let _ = fs::remove_dir_all(&temporary);
    assert!(!output.status.success());
    assert!(stderr(&output).contains("unconfigured GitHub repository"), "{}", stderr(&output));
}
