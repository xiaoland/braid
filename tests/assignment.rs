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

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut permissions = fs::metadata(path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("chmod executable");
    }
}

fn initialize(temporary: &Path) -> PathBuf {
    let source = temporary.join("source");
    let harness = temporary.join("harness");
    fs::create_dir_all(&source).expect("create source");
    fs::create_dir_all(&harness).expect("create harness");
    fs::write(source.join("README.md"), "demo\n").expect("write source");
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
    let pi = temporary.join("fake-pi");
    executable(&pi, "#!/bin/sh\necho 'pi 1.2.3'\n");
    let config = harness.join("config.toml");
    let output = braid()
        .args(["init", "--source"])
        .arg(&source)
        .args(["--config"])
        .arg(&config)
        .args(["--name", "demo", "--adapter", "pi", "--executable"])
        .arg(&pi)
        .args(["--model", "example-chat", "--llm-provider", "example-llm"])
        .output()
        .expect("initialize config");
    assert!(output.status.success(), "{}", stderr(&output));
    let output = braid()
        .args(["migrate", "apply", "--config"])
        .arg(&config)
        .output()
        .expect("apply migrations");
    assert!(output.status.success(), "{}", stderr(&output));
    config
}

#[test]
fn assignment_reactivation_queues_a_new_generation_for_a_blocked_issue() {
    let temporary =
        std::env::temp_dir().join(format!("braid-assignment-reactivate-{}", std::process::id()));
    let config = initialize(&temporary);
    let database = temporary.join("harness/state/braid.sqlite3");
    let connection = rusqlite::Connection::open(&database).expect("open database");
    connection
        .execute_batch(
            "INSERT INTO repositories(node_id,name_with_owner,observed_at) VALUES ('repo-demo','demo','2026-01-01T00:00:00Z');
             INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
             VALUES ('issue-1','repo-demo','issue',1,'open','2026-01-01T00:00:00Z');
             INSERT INTO assignments(assignment_id,work_item_node_id,generation,lifecycle,assigned_at,retired_at)
             VALUES ('assignment-blocked','issue-1',1,'blocked','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');",
        )
        .expect("seed blocked assignment");

    let output = braid()
        .args(["assignment", "reactivate", "1", "--kind", "issue", "--config"])
        .arg(&config)
        .output()
        .expect("reactivate blocked assignment");
    assert!(output.status.success(), "stdout:\n{}\nstderr:\n{}", stdout(&output), stderr(&output));
    assert!(stdout(&output).contains("new generation"), "{}", stdout(&output));

    let (kind, detail, lifecycle, trusted): (String, String, String, i64) = connection
        .query_row(
            "SELECT kind,detail,lifecycle,trusted_mention FROM events WHERE detail='operator_reactivation'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .expect("read reactivation event");
    assert_eq!(
        (kind.as_str(), detail.as_str(), lifecycle.as_str(), trusted),
        ("mention", "operator_reactivation", "pending", 0)
    );
    let batches: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM wake_batches wb
             JOIN wake_batch_events wbe ON wbe.batch_id=wb.batch_id
             JOIN events e ON e.event_id=wbe.event_id
             WHERE e.detail='operator_reactivation'",
            [],
            |row| row.get(0),
        )
        .expect("count wake batches");
    assert_eq!(batches, 1);

    let second = braid()
        .args(["assignment", "reactivate", "1", "--kind", "issue", "--config"])
        .arg(&config)
        .output()
        .expect("reactivate blocked assignment twice");
    assert!(second.status.success(), "{}", stderr(&second));
    let event_count: i64 = connection
        .query_row("SELECT COUNT(*) FROM events WHERE detail='operator_reactivation'", [], |row| {
            row.get(0)
        })
        .expect("count reactivation events");
    let _ = fs::remove_dir_all(&temporary);
    assert_eq!(event_count, 1);
}
