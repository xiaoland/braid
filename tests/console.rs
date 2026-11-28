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

fn assert_conflict(response: reqwest::blocking::Response, reason: &str) {
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().unwrap();
    assert!(body["error"]["message"].as_str().unwrap().contains(reason), "{body}");
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
    let settings: toml::Value = toml::from_str(&fs::read_to_string(&harness).unwrap()).unwrap();
    let _unavailable_health =
        TcpListener::bind(settings["server"]["health"].as_str().unwrap()).unwrap();
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
    let resources = client.get(format!("{endpoint}/api/resources")).send().unwrap();
    assert_eq!(resources.status(), StatusCode::SERVICE_UNAVAILABLE);
    let error: Value = resources.json().unwrap();
    assert!(error["error"].as_str().unwrap().contains("observation database unavailable"));
    // An empty protobuf message is a valid ExportLogsServiceRequest.
    let export = client
        .post(format!("{endpoint}/v1/logs"))
        .header("content-type", "application/x-protobuf")
        .send()
        .unwrap();
    assert_eq!(export.status(), StatusCode::SERVICE_UNAVAILABLE);

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

#[test]
#[allow(clippy::too_many_lines)]
fn object_cli_mints_on_first_write_and_platform_fences_a_delayed_mint() {
    let root = Scratch(std::env::temp_dir().join(format!("braid-writer-{}", uuid::Uuid::now_v7())));
    fs::create_dir(&root.0).unwrap();
    let (platform, harness) = configure(&root.0);
    let settings: toml::Value = toml::from_str(&fs::read_to_string(&platform).unwrap()).unwrap();
    let endpoint = format!("http://{}", settings["listen"].as_str().unwrap());
    let active_platform = Console(
        braid()
            .args(["platform", "serve", "--config"])
            .arg(&platform)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let client = Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap();
    wait_for("Platform listener", || {
        client
            .get(format!("{endpoint}/api/platform/repository"))
            .send()
            .ok()
            .filter(|response| response.status().is_success())
    });
    let object_command = |request_id: &str, command: Value| {
        let revision = command.get("number").map(|number| {
            get(&client, &endpoint, &format!("/api/control/items/{number}"))["item"]["revision"]
                .clone()
        });
        let receipt: Value = client
            .post(format!("{endpoint}/api/control/commands"))
            .json(&json!({"requestId":request_id, "command":command, "expectedRevision":revision}))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        assert_eq!(receipt["state"], "applied", "{receipt}");
    };
    object_command("create", json!({"type":"createIssue", "title":"Writer", "body":""}));
    object_command("assign", json!({"type":"assign", "actor":"issue-codex", "number":1}));
    let cursor =
        || get(&client, &endpoint, "/api/platform/events")["lastSequence"].as_i64().unwrap();
    let base = || {
        let mut command = braid();
        command.args(["object", "--config"]).arg(&harness).current_dir(root.0.join("source"));
        command
    };
    checked(base().env_remove("BRAID_AGENT").args(["show", "1"]));

    let database = rusqlite::Connection::open(root.0.join("harness/state/braid.sqlite3")).unwrap();
    database.execute_batch(
        "INSERT INTO owner_leases VALUES('runtime',1,'fixture','9999-12-31T00:00:00Z');
         INSERT INTO repositories VALUES('local:demo','demo',1,'now');
         INSERT INTO work_items(node_id,repository_node_id,kind,number,state,context_revision,observed_at)
           VALUES('item','local:demo','issue',1,'open','revision','now');
         INSERT INTO profiles VALUES('issue-codex',1,printf('%064d',0),'pi','[]');
         INSERT INTO assignments VALUES('assignment','item',1,'active','now',NULL);
         INSERT INTO agent_instances(agent_id,assignment_id,profile_id,profile_revision,role,lifecycle)
           VALUES('agent','assignment','issue-codex',1,'issue','idle');
         INSERT INTO provider_sessions(session_id,agent_id,provider_kind,provider_session_id,context_revision,instruction_revision,lifecycle,started_at)
           VALUES('session','agent','pi','native','revision','1','running','now');",
    ).unwrap();
    database.execute(
        "INSERT INTO worktrees(worktree_id,agent_id,path,repository_node_id,lifecycle,observed_at)
         VALUES('tree','agent',?1,'local:demo','active','now')",
        [root.0.join("source").to_str().unwrap()],
    ).unwrap();
    let execution_id = uuid::Uuid::now_v7().to_string();
    database.execute(
        "UPDATE provider_sessions SET builtin_execution_id=?1, builtin_execution_owner='fixture'
         WHERE session_id='session'",
        [&execution_id],
    ).unwrap();
    let authority_cursor = cursor();
    database
        .execute(
            "INSERT INTO turns(turn_id,session_id,context_revision,trigger_kind,lifecycle,
          builtin_writer_cursor,builtin_execution_id)
         VALUES('turn','session','revision','wake_batch','running',?1,?2)",
            rusqlite::params![authority_cursor, execution_id],
        )
        .unwrap();

    let client_id: String =
        database.query_row("SELECT client_id FROM builtin_upstream", [], |r| r.get(0)).unwrap();
    let epoch = || {
        let mut url = reqwest::Url::parse(&format!("{endpoint}/api/platform/grants")).unwrap();
        url.query_pairs_mut()
            .append_pair("clientId", &client_id)
            .append_pair("itemNumber", "1")
            .append_pair("actor", "issue-codex");
        client.get(url).send().unwrap().error_for_status().unwrap().json::<Value>().unwrap()
    };
    assert!(epoch().is_null());
    object_command("unrelated", json!({"type":"comment", "number":1,"body":"Human update"}));
    checked(base().env("BRAID_AGENT", "1").args(["comment", "1", "--body", "Agent update"]));
    let context = get(&client, &endpoint, "/api/platform/context/1");
    assert!(context["comments"].as_array().unwrap().iter().any(|comment| {
        comment["body"] == "Agent update" && comment["author"]["login"] == "issue-codex"
    }));
    assert_eq!(epoch()["active"], true);

    let fence = |id: &str| {
        client
            .post(format!("{endpoint}/api/platform/executions/fence"))
            .json(&json!({"executionId":id,"clientId":client_id}))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .unwrap()
    };
    let (request_id, grant_id, granted_epoch, token): (String, String, i64, String) = database
        .query_row(
            "SELECT request_id,grant_id,epoch,token FROM builtin_writer_grants WHERE session_id='native'",
            [],
            |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).unwrap();
    for _ in 0..2 {
        assert_eq!(fence(&execution_id), json!({"executionId":execution_id,"clientId":client_id}));
    }
    assert_eq!(epoch(), json!({"active":false,"epoch":granted_epoch}));

    // Fence after mint: old receipts and a valid old token cannot reopen execution.
    let prior_mint = json!({"requestId":request_id,"clientId":client_id,"actor":"issue-codex",
        "itemNumber":1,"expectedEpoch":granted_epoch-1,"authorityCursor":authority_cursor,
        "executionId":execution_id});
    let replay =
        client.post(format!("{endpoint}/api/platform/grants")).json(&prior_mint).send().unwrap();
    assert_conflict(replay, "execution attempt is fenced");
    let revision = get(&client, &endpoint, "/api/control/items/1")["item"]["revision"].clone();
    let old_write = client
        .post(format!("{endpoint}/api/control/commands"))
        .json(&json!({"requestId":"old-write","actor":"issue-codex","expectedRevision":revision,
          "writer":{"grantId":grant_id,"epoch":granted_epoch,"token":token},
          "command":{"type":"comment","number":1,"body":"Fenced turn"}}))
        .send()
        .unwrap();
    assert_conflict(old_write, "writer grant is stale, revoked");

    // Fence before mint: no corresponding publication row needs to exist.
    let late_execution = uuid::Uuid::now_v7().to_string();
    assert_eq!(fence(&late_execution)["executionId"], late_execution);
    let mint = json!({"requestId":"delayed-mint","clientId":client_id,"actor":"issue-codex",
        "itemNumber":1,"expectedEpoch":granted_epoch,"authorityCursor":cursor(),
        "executionId":late_execution});
    let late = client.post(format!("{endpoint}/api/platform/grants")).json(&mint).send().unwrap();
    assert_conflict(late, "execution attempt is fenced");
    assert_eq!(epoch(), json!({"active":false,"epoch":granted_epoch}));

    object_command(
        "configure-root",
        json!({"type":"configureRunRoot", "number":1,
        "deliveryRef":"refs/heads/braid/test", "runId":"test-run", "clientId":client_id}),
    );
    // The bound configuration remains the same after loss of the Git checkout.
    let source = root.0.join("source");
    let online = get(&client, &endpoint, "/api/platform/scope/1");
    assert_eq!(online["root"]["number"], 1);
    let last_online = cursor();
    fs::rename(source.join(".git"), source.join(".git.offline")).unwrap();
    drop(active_platform);
    let _reopened_platform = Console(
        braid()
            .args(["platform", "serve", "--config"])
            .arg(&platform)
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for("Platform reopening with Git offline", || {
        client
            .get(format!("{endpoint}/api/platform/repository"))
            .send()
            .ok()
            .filter(|response| response.status().is_success())
    });

    object_command(
        "git-offline-comment",
        json!({"type":"comment","number":1,"body":"Git-independent"}),
    );
    let events = get(&client, &endpoint, &format!("/api/platform/events?after={last_online}"));
    assert!(
        events["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["event"]["visible_body"] == "Git-independent")
    );
    assert_eq!(get(&client, &endpoint, "/api/platform/scope/1/boundary")["rootOpen"], true);
    let offline = client.get(format!("{endpoint}/api/platform/scope/1")).send().unwrap();
    assert_eq!(offline.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body: Value = offline.json().unwrap();
    assert_eq!(body["error"]["code"], "unavailable");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("could not find repository"),
        "{body}"
    );
}
