#![cfg(unix)]

use std::{
    fs,
    net::TcpListener,
    os::unix::fs::{PermissionsExt as _, symlink},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::Pid,
};
use prost::Message as _;
use reqwest::{StatusCode, blocking::Client};
use serde_json::{Value, json};

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Console(Child);

impl Drop for Console {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn braid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_braid"))
}

fn checked(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(output.status.success(), "{command:?}: {}", String::from_utf8_lossy(&output.stderr));
}

fn wait_for<T>(label: &str, mut read: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(result) = read() {
            return result;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn configure(root: &Path) -> (PathBuf, PathBuf) {
    let source = root.join("source");
    checked(Command::new("git").args(["init", "-q", "-b", "main"]).arg(&source));
    checked(Command::new("git").arg("-C").arg(&source).args([
        "-c",
        "user.email=braid@example.invalid",
        "-c",
        "user.name=Braid Test",
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--allow-empty",
        "-q",
        "-m",
        "init",
    ]));
    let platform_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let platform_address = platform_listener.local_addr().unwrap().to_string();
    let platform = root.join("platform.toml");
    checked(
        braid()
            .args(["platform", "init", "--name", "demo", "--listen", &platform_address, "--source"])
            .arg(&source)
            .arg("--config")
            .arg(&platform),
    );
    let executable = root.join("fake-pi");
    fs::write(&executable, "#!/bin/sh\n[ \"$1\" = --version ] || exit 99\necho 'pi 1.2.3'\n")
        .unwrap();
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
    let harness = root.join("harness/config.toml");
    checked(
        braid()
            .args([
                "init",
                "--name",
                "demo",
                "--adapter",
                "pi",
                "--model",
                "unused",
                "--llm-provider",
                "example-llm",
                "--llm-credential-kind",
                "none",
                "--llm-base-url",
                "https://llm.example.invalid/v1",
                "--endpoint",
            ])
            .arg(format!("http://{platform_address}"))
            .arg("--executable")
            .arg(executable)
            .arg("--source")
            .arg(&source)
            .arg("--config")
            .arg(&harness),
    );
    let ingress_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let health_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = fs::read_to_string(&harness)
        .unwrap()
        .replace("127.0.0.1:18080", &ingress_listener.local_addr().unwrap().to_string())
        .replace("127.0.0.1:18081", &health_listener.local_addr().unwrap().to_string());
    fs::write(&harness, config).unwrap();
    checked(braid().args(["migrate", "apply", "--config"]).arg(&harness));
    (platform, harness)
}

fn serve(platform: &Path, harness: &Path, database: &Path) -> Command {
    let mut command = braid();
    command
        .args(["console", "--listen", "127.0.0.1:0", "--database"])
        .arg(database)
        .arg("--platform-config")
        .arg(platform)
        .arg("--config")
        .arg(harness);
    command
}

#[derive(prost::Message)]
struct OtlpStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

fn assert_observation_unavailable(client: &Client, console: &str) {
    for path in [
        "/api/status",
        "/api/resources",
        "/api/records",
        "/api/batches/1",
        "/api/receive-errors",
        "/api/receive-errors/1",
    ] {
        let response = client.get(format!("{console}{path}")).send().unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{path}");
        let body: Value = response.json().unwrap();
        assert!(body["error"].as_str().unwrap().contains("observation database unavailable"));
    }
    for signal in ["traces", "logs", "metrics"] {
        // The default Export*ServiceRequest encodes as an empty protobuf message.
        let response = client
            .post(format!("{console}/v1/{signal}"))
            .header("content-type", "application/x-protobuf")
            .body(Vec::new())
            .send()
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE, "{signal}");
        assert_eq!(response.headers()["content-type"], "application/x-protobuf");
        let status = OtlpStatus::decode(response.bytes().unwrap()).unwrap();
        assert_eq!(status.code, 14);
        assert!(status.message.contains("observation database unavailable"));
    }
}

fn get(client: &Client, console: &str, path: &str) -> Value {
    client
        .get(format!("{console}{path}"))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .unwrap()
}

fn runtime_command(client: &Client, console: &str, command: &Value) {
    let response =
        client.post(format!("{console}/api/runtime/commands")).json(command).send().unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED, "{}", response.text().unwrap());
}

#[test]
fn console_controls_keep_working_when_the_observation_database_cannot_open() {
    let root =
        Scratch(std::env::temp_dir().join(format!("braid-console-{}", uuid::Uuid::now_v7())));
    fs::create_dir(&root.0).unwrap();
    let (platform, harness) = configure(&root.0);
    let original_config = fs::read(&harness).unwrap();
    let alias = root.0.join("aliased.sqlite3");
    symlink(root.0.join("harness/state/braid.sqlite3"), &alias).unwrap();
    let rejected = serve(&platform, &harness, &alias).output().unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("databases must be separate"));

    let observations = root.0.join("observations.sqlite3");
    fs::create_dir(&observations).unwrap();
    let stdout = root.0.join("console.stdout");
    let stderr = root.0.join("console.stderr");
    let mut console = Console(
        serve(&platform, &harness, &observations)
            .stdout(Stdio::from(fs::File::create(&stdout).unwrap()))
            .stderr(Stdio::from(fs::File::create(&stderr).unwrap()))
            .spawn()
            .unwrap(),
    );
    let endpoint = wait_for("Console listener", || {
        assert!(
            console.0.try_wait().unwrap().is_none(),
            "{}",
            fs::read_to_string(&stderr).unwrap()
        );
        fs::read_to_string(&stdout)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("Braid Console ").map(str::to_owned))
    });
    let client = Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();
    let capabilities = get(&client, &endpoint, "/api/console");
    assert_eq!(capabilities["platform"]["managed"], true);
    assert_eq!(capabilities["harness"]["configured"], true);
    assert_observation_unavailable(&client, &endpoint);

    let issue = client
        .post(format!("{endpoint}/api/control/commands"))
        .json(&json!({"requestId":"create", "command":{"type":"createIssue", "title":"Independent issue", "body":""}}))
        .send()
        .unwrap()
        .error_for_status()
        .unwrap()
        .json::<Value>()
        .unwrap();
    assert_eq!(issue["state"], "applied", "{issue}");
    runtime_command(
        &client,
        &endpoint,
        &json!({"request_id":"start", "operation":"start", "transport_only":true}),
    );
    wait_for("platform event in the Harness ledger", || {
        let snapshot = get(&client, &endpoint, "/api/harness/snapshot");
        (snapshot["upstream"][0]["cursor"].as_u64().unwrap_or_default() > 0).then_some(())
    });
    let running = get(&client, &endpoint, "/api/runtime");
    assert_eq!(running["instance"], "demo");
    assert_eq!(running["provider_enabled"], false);
    assert_eq!(running["stop_confirmed"], false);
    runtime_command(
        &client,
        &endpoint,
        &json!({"request_id":"stop", "operation":"stop", "expected_generation":running["generation"]}),
    );
    wait_for("confirmed Harness stop", || {
        let status = get(&client, &endpoint, "/api/runtime");
        assert_ne!(status["state"], "blocked", "{status}");
        (status["stop_confirmed"] == true).then_some(())
    });
    let receipt = get(&client, &endpoint, "/api/runtime/receipts/stop");
    assert_eq!(receipt["result"]["stop_confirmed"], true, "{receipt}");
    assert_eq!(get(&client, &endpoint, "/api/control/receipts/create"), issue);
    assert_eq!(fs::read(&harness).unwrap(), original_config);

    kill(Pid::from_raw(i32::try_from(console.0.id()).unwrap()), Signal::SIGTERM).unwrap();
    let result = wait_for("graceful Console exit", || console.0.try_wait().unwrap());
    assert!(result.success(), "{}", fs::read_to_string(stderr).unwrap());
}
