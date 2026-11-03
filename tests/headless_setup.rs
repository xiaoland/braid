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

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod executable");
    }
}

fn fake_bin() -> PathBuf {
    let directory = std::env::temp_dir().join(format!("braid-headless-bin-{}", std::process::id()));
    fs::create_dir_all(&directory).expect("create fake bin");
    write_executable(
        &directory.join("gh"),
        r#"#!/bin/sh
if [ "$1" = "api" ] && [ "$2" = "user" ]; then
  echo testuser
  exit 0
fi
case "$2" in
  app-manifests/*/conversions)
    cat <<'JSON'
{"id":123456,"slug":"braid-of-testuser","name":"braid-of-testuser","pem":"-----BEGIN PRIVATE KEY-----\nTEST\n-----END PRIVATE KEY-----\n","webhook_secret":"webhook","html_url":"https://github.com/apps/braid-of-testuser"}
JSON
    exit 0
    ;;
esac
echo "unexpected gh call: $*" >&2
exit 1
"#,
    );
    write_executable(&directory.join("pi"), "#!/bin/sh\necho 'pi 1.2.3'\n");
    write_executable(
        &directory.join("git"),
        r#"#!/bin/sh
if [ "$1" = "clone" ]; then
  for destination in "$@"; do :; done
  mkdir -p "$destination/.git"
  exit 0
fi
echo "unexpected git call: $*" >&2
exit 1
"#,
    );
    directory
}

fn setup_command(fake_bin: &Path, home: &Path) -> Command {
    let mut command = braid();
    let path = format!("{}:{}", fake_bin.display(), std::env::var("PATH").unwrap_or_default());
    command
        .env("PATH", path)
        .args(["setup", "testuser/repository", "--adapter", "pi", "--instance", "demo"])
        .arg("--user-home")
        .arg(home);
    command
}

#[test]
fn no_browser_setup_can_be_completed_from_callback_url() {
    let temporary =
        std::env::temp_dir().join(format!("braid-headless-setup-{}", std::process::id()));
    let home = temporary.join("home");
    let fake_bin = fake_bin();

    let output = setup_command(&fake_bin, &home)
        .args(["--no-browser"])
        .output()
        .expect("start no-browser setup");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("--callback-url"), "{}", stdout(&output));

    let draft_path = home.join("setup-drafts/demo.toml");
    let draft = fs::read_to_string(&draft_path).expect("read setup draft");
    assert!(draft.contains("repository = \"testuser/repository\""), "{draft}");
    let state = draft
        .lines()
        .find_map(|line| line.strip_prefix("state = \"")?.strip_suffix('\"'))
        .expect("draft state")
        .to_owned();

    let wrong = setup_command(&fake_bin, &home)
        .args(["--model", "example-chat", "--llm-provider", "example-llm"])
        .args(["--callback-url", "http://127.0.0.1/callback?code=wrong&state=wrong"])
        .output()
        .expect("reject mismatched callback state");
    assert!(!wrong.status.success());
    assert!(stderr(&wrong).contains("state does not match"), "{}", stderr(&wrong));

    let callback = format!("http://127.0.0.1/callback?code=code123&state={state}");
    let output = setup_command(&fake_bin, &home)
        .args(["--model", "example-chat", "--llm-provider", "example-llm"])
        .args(["--runtime-executable"])
        .arg(fake_bin.join("pi"))
        .args(["--callback-url", &callback])
        .output()
        .expect("complete headless setup");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(!draft_path.exists());

    let config = home.join("instances/demo/config.toml");
    assert!(config.is_file());
    let output = braid()
        .args(["config", "check", "--config"])
        .arg(&config)
        .output()
        .expect("check completed config");
    let _ = fs::remove_dir_all(&temporary);
    let _ = fs::remove_dir_all(&fake_bin);
    assert!(output.status.success(), "{}", stderr(&output));
}
