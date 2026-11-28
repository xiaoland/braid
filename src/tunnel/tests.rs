use super::*;

#[cfg(unix)]
async fn start_fake_tunnel(executable: &Path) -> Result<QuickTunnel, TunnelError> {
    let (_stop, mut shutdown) = watch::channel(false);
    QuickTunnel::start(executable, "http://127.0.0.1:1", &mut shutdown)
        .await
        .map(|candidate| candidate.expect("fake tunnel was not asked to stop"))
}

#[cfg(unix)]
#[tokio::test]
async fn tunnel_not_found_is_detected_while_the_child_remains_alive() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = std::env::temp_dir().join(format!("braid-tunnel-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join("wrangler");
    std::fs::write(&executable,"#!/bin/sh\nprintf '%s\\n' 'https://test.trycloudflare.com' 'Registered tunnel connection'\nprintf '%s\\n' 'ERR Unauthorized: Tunnel not found' >&2\nexec /usr/bin/tail -f /dev/null\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut tunnel = start_fake_tunnel(&executable).await.unwrap();
    if tunnel.failure().is_none() {
        tokio::time::timeout(Duration::from_secs(2), tunnel.failure.changed())
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(tunnel.failure(), Some("tunnel_not_found"));
    assert!(!tunnel.has_exited().unwrap());
    tunnel.stop().await.unwrap();
    std::fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum Registration {
    Success,
    Failure,
    Pending,
}

#[cfg(unix)]
fn tree_script(root: &std::path::Path, registration: Registration) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(root).unwrap();
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let script = root.join("wrangler");
    let output = if matches!(registration, Registration::Success) {
        "printf '%s\\n' 'https://tree.trycloudflare.com' 'Registered tunnel connection'"
    } else {
        "printf '%s\\n' 'starting without a public URL'"
    };
    let ending = if matches!(registration, Registration::Failure) { "exit 0" } else { "wait" };
    let middle = format!(
        "trap 'wait; exit 0' TERM; /usr/bin/tail -f /dev/null & echo $! > \"$leaf_file\"; {output}; wait"
    );
    std::fs::write(&script,format!("#!/bin/sh\ntrap 'wait; exit 0' TERM\necho $$ > {}\nexport leaf_file={}\n/bin/sh -c {} &\n{ending}\n",quote(root.join("wrapper.pid").to_str().unwrap()),quote(root.join("leaf.pid").to_str().unwrap()),quote(&middle))).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    script
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_a_tunnel_reaps_the_owned_wrapper_and_grandchild_group() {
    use nix::{
        errno::Errno,
        sys::signal::{kill, killpg},
        unistd::{Pid, getpgrp},
    };
    let root = std::env::temp_dir().join(format!("braid-tunnel-tree-{}", Uuid::now_v7()));
    let script = tree_script(&root, Registration::Success);
    let tunnel = start_fake_tunnel(&script).await.unwrap();
    let group = Pid::from_raw(i32::try_from(tunnel.child.id().unwrap()).unwrap());
    let leaf = Pid::from_raw(
        std::fs::read_to_string(root.join("leaf.pid")).unwrap().trim().parse().unwrap(),
    );
    assert_ne!(group, getpgrp(), "the Tunnel must not share Braid's process group");
    assert!(kill(leaf, None).is_ok());
    tunnel.stop().await.unwrap();
    assert_eq!(killpg(group, None), Err(Errno::ESRCH));
    assert_eq!(kill(leaf, None), Err(Errno::ESRCH));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn failed_registration_also_closes_the_owned_process_tree() {
    use nix::{errno::Errno, sys::signal::killpg, unistd::Pid};
    let root = std::env::temp_dir().join(format!("braid-tunnel-failed-tree-{}", Uuid::now_v7()));
    let script = tree_script(&root, Registration::Failure);
    assert!(matches!(start_fake_tunnel(&script).await, Err(TunnelError::MissingUrl)));
    let group = Pid::from_raw(
        std::fs::read_to_string(root.join("wrapper.pid")).unwrap().trim().parse().unwrap(),
    );
    assert_eq!(killpg(group, None), Err(Errno::ESRCH));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_during_registration_confirms_the_tunnel_process_group_stopped() {
    use nix::{errno::Errno, sys::signal::killpg, unistd::Pid};
    let root = std::env::temp_dir().join(format!("braid-tunnel-registration-{}", Uuid::now_v7()));
    let script = tree_script(&root, Registration::Pending);
    let (stop, mut shutdown) = watch::channel(false);
    let candidate = tokio::spawn(async move {
        QuickTunnel::start(&script, "http://127.0.0.1:1", &mut shutdown).await
    });
    let group = timeout(Duration::from_secs(2), async {
        loop {
            if let Ok(pid) = std::fs::read_to_string(root.join("wrapper.pid"))
                && let Ok(pid) = pid.trim().parse()
                && root.join("leaf.pid").exists()
            {
                return Pid::from_raw(pid);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(killpg(group, None).is_ok());
    stop.send(true).unwrap();
    assert!(candidate.await.unwrap().unwrap().is_none());
    assert_eq!(killpg(group, None), Err(Errno::ESRCH));
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn closing_reaps_an_exited_wrapper_but_waits_for_remaining_group_members() {
    use nix::{
        errno::Errno,
        sys::signal::{Signal, kill, killpg},
        unistd::Pid,
    };

    // Both children belong to this test. Keeping the second one unreaped makes
    // the otherwise transient zombie-only group deterministic, without mocks.
    let mut wrapper = Command::new("/usr/bin/tail")
        .args(["-f", "/dev/null"])
        .process_group(0)
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut group = process_group::OwnedGroup::new(&wrapper);
    let wrapper_pid = wrapper.id().unwrap();
    let group_id = Pid::from_raw(i32::try_from(wrapper_pid).unwrap());
    let mut member = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .process_group(group_id.as_raw())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let member_pid = member.id().unwrap();
    wait_for_process_state(member_pid, "Z").await;
    kill(group_id, Signal::SIGTERM).unwrap();
    wait_for_process_state(wrapper_pid, "Z").await;

    let close = group.close(&mut wrapper);
    tokio::pin!(close);
    tokio::select! {
        biased;
        result = &mut close => panic!("cleanup completed with an unreaped member: {result:?}"),
        () = wait_for_process_state(wrapper_pid, "") => {}
    }
    // Reaping the wrapper alone is insufficient. The other real member still
    // owns the group until its parent (this test) consumes its exit status.
    assert_ne!(killpg(group_id, None), Err(Errno::ESRCH));
    assert!(member.wait().await.unwrap().success());
    tokio::time::timeout(Duration::from_secs(2), &mut close).await.unwrap().unwrap();
    assert_eq!(killpg(group_id, None), Err(Errno::ESRCH));
}

#[cfg(unix)]
async fn wait_for_process_state(pid: u32, expected: &str) {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let output = Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "stat="])
                .output()
                .await
                .unwrap();
            let state = String::from_utf8(output.stdout).unwrap();
            let state = state.trim();
            if (expected.is_empty() && state.is_empty())
                || (!expected.is_empty() && state.starts_with(expected))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn signed_probe_requires_ingress_acceptance_not_an_arbitrary_http_success() {
    use crate::{
        producer::{IngressState, webhook_handler},
        store::{SchedulerPolicy, StoreActor},
    };
    use axum::{http::StatusCode, routing::post};
    use std::sync::Arc;
    let root = std::env::temp_dir().join(format!("braid-tunnel-probe-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("probe.sqlite3");
    let store = StoreActor::start(database.clone(), root.join("backups")).unwrap();
    store.apply().unwrap();
    let app = axum::Router::new()
        .route("/proxy", post(|| async { StatusCode::OK }))
        .route("/webhook", post(webhook_handler))
        .with_state(Arc::new(IngressState {
            store: Arc::new(store),
            policy: SchedulerPolicy { quiet_seconds: 30, event_threshold: 8 },
            repositories: vec!["owner/repo".into()],
            webhook_secret: Arc::new(b"secret".to_vec()),
            handle: "braid".into(),
            app_actor_node_id: "APP".into(),
            app_actor_login: "braid[bot]".into(),
            agent_actor_node_ids: vec![],
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let error = signed_public_probe_once(&format!("{base}/proxy"), b"secret", "owner/repo", "R1")
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("HTTP 200 OK, expected 202"), "{error}");
    signed_public_probe_once(&format!("{base}/webhook"), b"secret", "owner/repo", "R1")
        .await
        .unwrap();
    let count: i64 = rusqlite::Connection::open(database).unwrap().query_row(
        "SELECT COUNT(*) FROM deliveries WHERE event_name='ping' AND repository_node_id='R1' AND repository_name='owner/repo' AND delivery_guid != ''",
        [],
        |row| row.get(0),
    ).unwrap();
    assert_eq!(count, 1, "signed ping must be durably admitted for the selected repository");
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(root).unwrap();
}
