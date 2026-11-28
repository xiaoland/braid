use std::{
    fs,
    io::{Read as _, Write as _},
    net::TcpListener,
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

fn fake_bin(temporary: &Path) -> PathBuf {
    let directory = temporary.join("bin");
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
    printf '%s\n' "$2" >> "$BRAID_TEST_CONVERSIONS"
    cat <<'JSON'
{"id":123456,"slug":"braid-of-testuser","name":"braid-of-testuser","pem":"-----BEGIN PRIVATE KEY-----\nTEST\n-----END PRIVATE KEY-----\n","webhook_secret":"webhook","html_url":"https://github.com/apps/braid-of-testuser"}
JSON
    exit 0
    ;;
  users/testuser|orgs/testuser)
    printf '{"avatar_url":"%s"}\n' "$BRAID_TEST_AVATAR_URL"
    exit 0
    ;;
esac
echo "unexpected gh call: $*" >&2
exit 1
"#,
    );
    write_executable(&directory.join("pi"), "#!/bin/sh\necho 'pi 1.2.3'\n");
    for browser in ["open", "xdg-open"] {
        write_executable(&directory.join(browser), "#!/bin/sh\ntouch \"$BRAID_TEST_BROWSER\"\n");
    }
    write_executable(&directory.join("wrangler"), "#!/bin/sh\necho 'wrangler 0.0.0-test'\n");
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
        .env("TMPDIR", fake_bin)
        .env("BRAID_TEST_BROWSER", fake_bin.join("browser-opened"))
        .env("BRAID_TEST_CONVERSIONS", fake_bin.join("conversions"))
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
    let fake_bin = fake_bin(&temporary);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind avatar server");
    let avatar_url =
        format!("http://{}/avatar.png", listener.local_addr().expect("avatar address"));
    let avatar_server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept avatar request");
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).expect("read avatar request");
        let png = include_bytes!(
            "../docs/assets/logo/png/braid-color-on-transparent/braid-color-on-transparent-512.png"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            png.len()
        )
        .expect("write avatar response");
        stream.write_all(png).expect("write avatar image");
    });

    let output = setup_command(&fake_bin, &home)
        .args(["--no-browser"])
        .output()
        .expect("start no-browser setup");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("--callback-url"), "{}", stdout(&output));

    let draft_path = home.join("setup-drafts/demo.toml");
    let draft = fs::read_to_string(&draft_path).expect("read setup draft");
    let draft: toml::Value = toml::from_str(&draft).expect("decode saved draft");
    assert_eq!(draft["repository"].as_str(), Some("testuser/repository"));
    let state = draft["state"].as_str().expect("saved callback state");

    let wrong = setup_command(&fake_bin, &home)
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
        .args(["--callback-url", "http://127.0.0.1/callback?code=wrong&state=wrong"])
        .output()
        .expect("reject mismatched callback state");
    assert!(!wrong.status.success());
    assert!(stderr(&wrong).contains("state does not match"), "{}", stderr(&wrong));

    let conversions = fake_bin.join("conversions");
    assert!(!conversions.exists(), "invalid state must leave the one-use code unconsumed");
    let callback = format!("http://127.0.0.1/callback?code=code123&state={state}");
    let output = setup_command(&fake_bin, &home)
        .current_dir(&fake_bin)
        .env("BRAID_TEST_AVATAR_URL", avatar_url)
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
        .args(["--runtime-executable"])
        .arg("./pi")
        .args(["--callback-url", &callback])
        .output()
        .expect("complete headless setup");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(!draft_path.exists());
    assert!(home.join("instances/demo/braid-of-testuser-logo.png").is_file());
    avatar_server.join().expect("avatar server");
    assert!(!fake_bin.join("browser-opened").exists());
    assert_eq!(fs::read_to_string(conversions).unwrap(), "app-manifests/code123/conversions\n");
    assert_eq!(
        fs::read_to_string(home.join("instances/demo/secrets.toml")).expect("webhook secret"),
        "webhook_secret = \"webhook\"\n"
    );
    assert!(stdout(&output).contains("braid migrate apply --instance demo"));

    let output = braid()
        .env_remove("BRAID_CONFIG")
        .env_remove("BRAID_INSTANCE_HOME")
        .env("BRAID_USER_HOME", &home)
        .args(["config", "check", "--instance", "demo", "--json"])
        .output()
        .expect("resolve the newly registered instance");
    assert!(output.status.success(), "{}", stderr(&output));
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(summary["instance_key"], "demo");
    assert_eq!(summary["repository"], "testuser/repository");
    let _ = fs::remove_dir_all(&temporary);
}

#[test]
fn setup_rejects_local_runtime_failures_before_consuming_the_manifest() {
    let temporary =
        std::env::temp_dir().join(format!("braid-setup-preflight-{}", std::process::id()));
    let home = temporary.join("home");
    let fake_bin = fake_bin(&temporary);
    fs::create_dir_all(home.join("setup-drafts")).expect("create setup drafts");
    fs::write(
        home.join("setup-drafts/demo.toml"),
        "repository = \"testuser/repository\"\ninstance_key = \"demo\"\nstate = \"expected\"\n",
    )
    .expect("write draft");
    let converted = temporary.join("converted");
    for (runtime, expected) in [
        ("#!/bin/sh\necho 'pi 1.2.3'\nexit 1\n", "--version exited"),
        ("#!/bin/sh\nexit 0\n", "--version returned no version"),
    ] {
        write_executable(&fake_bin.join("pi"), runtime);
        let output = setup_command(&fake_bin, &home)
            .env("BRAID_TEST_CONVERSIONS", &converted)
            .args([
                "--model",
                "example-chat",
                "--llm-provider",
                "example-llm",
                "--llm-credential-kind",
                "none",
                "--llm-base-url",
                "https://llm.example.invalid/v1",
                "--callback-url",
                "http://127.0.0.1/callback?code=once&state=expected",
                "--runtime-executable",
            ])
            .arg(fake_bin.join("pi"))
            .output()
            .expect("run invalid runtime setup");
        assert!(!output.status.success());
        assert!(stderr(&output).contains(expected), "{}", stderr(&output));
        assert!(!converted.exists());
        assert!(!home.join("instances/demo").exists());
    }
    let _ = fs::remove_dir_all(&temporary);
}
