use std::{fs, process::Command};

fn braid() -> Command {
    Command::new(env!("CARGO_BIN_EXE_braid"))
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn assignment_reactivation_queues_one_wake_and_preserves_the_blocked_generation() {
    let temporary =
        std::env::temp_dir().join(format!("braid-assignment-reactivate-{}", uuid::Uuid::now_v7()));
    fs::create_dir_all(&temporary).expect("create test directory");
    let config = temporary.join("config.toml");
    fs::write(&config, include_str!("../config.example.toml")).expect("write config");
    let output = braid()
        .args(["migrate", "apply", "--config"])
        .arg(&config)
        .output()
        .expect("apply migrations");
    assert!(output.status.success(), "{}", stderr(&output));

    let database = temporary.join("state/braid.sqlite3");
    let connection = rusqlite::Connection::open(&database).expect("open database");
    connection
        .execute_batch(
            "INSERT INTO repositories(node_id,name_with_owner,observed_at) VALUES ('repo-demo','owner/repository','2026-01-01T00:00:00Z');
             INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
             VALUES ('issue-1','repo-demo','issue',1,'open','2026-01-01T00:00:00Z');
             INSERT INTO assignments(assignment_id,work_item_node_id,generation,lifecycle,assigned_at,retired_at)
             VALUES ('assignment-blocked','issue-1',1,'blocked','2026-01-01T00:00:00Z','2026-01-01T00:00:00Z');",
        )
        .expect("seed blocked assignment");

    let mut first_receipt = None;
    for _ in 0..2 {
        let output = braid()
            .args(["assignment", "reactivate", "1", "--kind", "issue", "--config"])
            .arg(&config)
            .output()
            .expect("reactivate blocked assignment");
        assert!(
            output.status.success(),
            "stdout:\n{}\nstderr:\n{}",
            stdout(&output),
            stderr(&output)
        );
        let events: Vec<_> = connection
            .prepare(
                "SELECT event_id,kind,lifecycle,trusted_mention FROM events
             WHERE work_item_node_id='issue-1' AND detail='operator_reactivation'",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let [(event, kind, lifecycle, trusted)] = events.as_slice() else {
            panic!("expected exactly one operator reactivation event, got {events:?}");
        };
        if let Some(previous) = &first_receipt {
            assert_eq!(event, previous, "retry must reuse the first durable event");
        } else {
            first_receipt = Some(event.clone());
        }
        assert!(stdout(&output).contains(event), "CLI receipt must identify the durable event");
        assert_eq!((kind.as_str(), lifecycle.as_str(), *trusted), ("mention", "pending", 0));
        let batches: i64 = connection.query_row(
            "SELECT COUNT(*) FROM wake_batch_events e JOIN wake_batches b ON b.batch_id=e.batch_id
             WHERE e.event_id=?1 AND b.work_item_node_id='issue-1'",
            [event], |row| row.get(0),
        ).unwrap();
        assert_eq!(batches, 1);
    }
    let assignment: (i64, String) = connection.query_row(
        "SELECT generation,lifecycle FROM assignments WHERE work_item_node_id='issue-1' ORDER BY generation DESC LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(assignment, (1, "blocked".into()));
    fs::remove_dir_all(temporary).unwrap();
}
