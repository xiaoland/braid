#![allow(dead_code)]

#[path = "../src/worktree.rs"]
mod worktree;

use std::path::Path;

fn initialize(path: &Path) {
    let repository = git2::Repository::init(path).unwrap();
    repository.set_head("refs/heads/main").unwrap();
    let tree_id = repository.index().unwrap().write_tree().unwrap();
    let tree = repository.find_tree(tree_id).unwrap();
    let signature = git2::Signature::now("Braid Test", "braid@example.invalid").unwrap();
    repository.commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[]).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn existing_target_must_share_the_requested_source_repository() {
    let root = std::env::temp_dir().join(format!("braid-worktree-{}", uuid::Uuid::now_v7()));
    let source = root.join("source");
    let wrong = root.join("wrong-repository");
    let target = root.join("actual-worktree");
    initialize(&source);
    initialize(&wrong);
    let git = which::which("git").unwrap();
    let mut request = worktree::WorktreeRequest {
        source: &source,
        target: &wrong,
        repository: "owner/repository",
        remote: "origin",
        local_only: true,
        git: &git,
        head_ref: "main",
        local_branch: "main",
    };
    assert!(matches!(
        worktree::provision(&request),
        Err(worktree::WorktreeError::TargetConflict(_))
    ));

    request.target = &target;
    request.local_branch = "agent";
    let created = worktree::provision(&request).unwrap();
    assert_eq!(created.path, target.canonicalize().unwrap());
    assert_eq!(worktree::provision(&request).unwrap().path, created.path);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn github_remote_identity_is_exact() {
    for remote in [
        "https://github.com/Owner/Repo.git",
        "ssh://git@github.com/owner/repo.git",
        "git@github.com:owner/repo.git",
    ] {
        assert_eq!(worktree::normalized_github_repository(remote).as_deref(), Some("owner/repo"));
    }
    for remote in
        ["https://example.invalid/owner/repo.git", "https://github.com/owner/repo/issues/1"]
    {
        assert_eq!(worktree::normalized_github_repository(remote), None);
    }
    assert_ne!(
        worktree::normalized_github_repository("https://github.com/owner/repo-backup.git")
            .as_deref(),
        Some("owner/repo")
    );
}
