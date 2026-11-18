use super::*;

#[cfg(unix)]
#[tokio::test]
async fn tunnel_not_found_is_detected_while_the_child_remains_alive() {
    use std::os::unix::fs::PermissionsExt as _;
    let directory = std::env::temp_dir().join(format!("braid-tunnel-{}", Uuid::now_v7()));
    std::fs::create_dir_all(&directory).unwrap();
    let executable = directory.join("wrangler");
    std::fs::write(&executable,"#!/bin/sh\nprintf '%s\\n' 'https://test.trycloudflare.com' 'Registered tunnel connection'\nprintf '%s\\n' 'ERR Unauthorized: Tunnel not found' >&2\nexec /usr/bin/tail -f /dev/null\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut tunnel = QuickTunnel::start(&executable, "http://127.0.0.1:1").await.unwrap();
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
fn tree_script(root: &std::path::Path, register: bool) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(root).unwrap();
    let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
    let script = root.join("wrangler");
    let output = if register {
        "printf '%s\\n' 'https://tree.trycloudflare.com' 'Registered tunnel connection'"
    } else {
        "printf '%s\\n' 'starting without a public URL'"
    };
    let ending = if register { "wait" } else { "exit 0" };
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
    let script = tree_script(&root, true);
    let tunnel = QuickTunnel::start(&script, "http://127.0.0.1:1").await.unwrap();
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
    let script = tree_script(&root, false);
    assert!(QuickTunnel::start(&script, "http://127.0.0.1:1").await.is_err());
    let group = Pid::from_raw(
        std::fs::read_to_string(root.join("wrapper.pid")).unwrap().trim().parse().unwrap(),
    );
    assert_eq!(killpg(group, None), Err(Errno::ESRCH));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn signed_probe_requires_ingress_acceptance_not_an_arbitrary_http_success() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let counter = Arc::new(AtomicUsize::new(0));
    let state = counter.clone();
    let app = axum::Router::new().route(
        "/webhook",
        axum::routing::post(move || {
            let counter = state.clone();
            async move {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    axum::http::StatusCode::OK
                } else {
                    axum::http::StatusCode::ACCEPTED
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/webhook", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    assert!(signed_public_probe_once(&url, b"secret", "owner/repo", "repo").await.is_err());
    signed_public_probe_once(&url, b"secret", "owner/repo", "repo").await.unwrap();
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    server.abort();
}
