use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::process::Command;

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("worktree source {0} is not a Git checkout")]
    NotRepository(PathBuf),
    #[error("worktree target {0} already exists but is not the requested checkout")]
    TargetConflict(PathBuf),
    #[error("Git command failed: {0}")]
    Git(String),
    #[error("worktree path is not valid UTF-8: {0}")]
    NonUtf8(PathBuf),
    #[error("cannot prepare worktree directory {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

#[derive(Debug, Clone)]
pub struct WorktreeRequest<'a> {
    pub git: &'a Path,
    pub source: &'a Path,
    pub target: &'a Path,
    pub repository: &'a str,
    pub remote: &'a str,
    pub head_ref: &'a str,
    pub local_branch: &'a str,
}

#[derive(Debug, Clone)]
pub struct ProvisionedWorktree {
    pub source: PathBuf,
    pub path: PathBuf,
    pub head_ref: String,
    pub local_branch: String,
}

pub async fn provision(
    request: &WorktreeRequest<'_>,
) -> Result<ProvisionedWorktree, WorktreeError> {
    let source = canonical_repository(request.git, request.source).await?;
    let remote_url =
        git_output(request.git, &source, &["remote", "get-url", request.remote]).await?;
    if normalized_github_repository(remote_url.trim()).as_deref()
        != Some(&request.repository.to_ascii_lowercase())
    {
        return Err(WorktreeError::Git(format!(
            "remote {} does not identify GitHub repository {}",
            request.remote, request.repository
        )));
    }
    if request.target.exists() {
        return verify_existing(request, &source).await;
    }
    if let Some(parent) = request.target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| WorktreeError::Io { path: parent.to_path_buf(), source })?;
    }
    git(
        request.git,
        &source,
        &[
            "fetch",
            "--no-tags",
            request.remote,
            &format!(
                "+refs/heads/{}:refs/remotes/{}/{}",
                request.head_ref, request.remote, request.head_ref
            ),
        ],
    )
    .await?;
    let target = path_text(request.target)?;
    git(
        request.git,
        &source,
        &[
            "worktree",
            "add",
            "-b",
            request.local_branch,
            target,
            &format!("refs/remotes/{}/{}", request.remote, request.head_ref),
        ],
    )
    .await?;
    verify_existing(request, &source).await
}

async fn verify_existing(
    request: &WorktreeRequest<'_>,
    source: &Path,
) -> Result<ProvisionedWorktree, WorktreeError> {
    let target_root = canonical_repository(request.git, request.target)
        .await
        .map_err(|_| WorktreeError::TargetConflict(request.target.to_path_buf()))?;
    let expected = request
        .target
        .canonicalize()
        .map_err(|source| WorktreeError::Io { path: request.target.to_path_buf(), source })?;
    if target_root != expected {
        return Err(WorktreeError::TargetConflict(request.target.to_path_buf()));
    }
    let branch = git_output(request.git, request.target, &["branch", "--show-current"]).await?;
    if branch.trim() != request.local_branch {
        return Err(WorktreeError::TargetConflict(request.target.to_path_buf()));
    }
    Ok(ProvisionedWorktree {
        source: source.to_path_buf(),
        path: expected,
        head_ref: request.head_ref.to_owned(),
        local_branch: request.local_branch.to_owned(),
    })
}

async fn canonical_repository(git_path: &Path, path: &Path) -> Result<PathBuf, WorktreeError> {
    if !path.is_dir() {
        return Err(WorktreeError::NotRepository(path.to_path_buf()));
    }
    let output = git_output(git_path, path, &["rev-parse", "--show-toplevel"])
        .await
        .map_err(|_| WorktreeError::NotRepository(path.to_path_buf()))?;
    PathBuf::from(output.trim())
        .canonicalize()
        .map_err(|source| WorktreeError::Io { path: path.to_path_buf(), source })
}

async fn git(git_path: &Path, cwd: &Path, arguments: &[&str]) -> Result<(), WorktreeError> {
    git_output(git_path, cwd, arguments).await.map(|_| ())
}

async fn git_output(
    git_path: &Path,
    cwd: &Path,
    arguments: &[&str],
) -> Result<String, WorktreeError> {
    let output = Command::new(git_path)
        .args(arguments)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| WorktreeError::Git(error.to_string()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(WorktreeError::Git(stderr.trim().to_owned()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn path_text(path: &Path) -> Result<&str, WorktreeError> {
    path.to_str().ok_or_else(|| WorktreeError::NonUtf8(path.to_path_buf()))
}

fn normalized_github_repository(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("git@github.com:"))?;
    let repository = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    (repository.split('/').count() == 2).then(|| repository.to_ascii_lowercase())
}
