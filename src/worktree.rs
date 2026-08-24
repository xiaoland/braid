use std::path::{Path, PathBuf};

use git2::{ErrorClass, ErrorCode, Repository, WorktreeAddOptions};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorktreeError {
    #[error("worktree source {0} is not a Git checkout")]
    NotRepository(PathBuf),
    #[error("worktree target {0} already exists but is not the requested checkout")]
    TargetConflict(PathBuf),
    #[error("Git operation failed: {0}")]
    Git(String),
    #[error("cannot prepare worktree directory {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

impl From<git2::Error> for WorktreeError {
    fn from(error: git2::Error) -> Self {
        Self::Git(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct WorktreeRequest<'a> {
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

pub fn provision(
    request: &WorktreeRequest<'_>,
) -> Result<ProvisionedWorktree, WorktreeError> {
    let source = tokio::task::block_in_place(|| canonical_repository(request.source))?;
    let remote_url = tokio::task::block_in_place(|| {
        let repo = Repository::open(&source)?;
        let remote = repo.find_remote(request.remote)?;
        Ok::<String, WorktreeError>(remote.url()?.to_owned())
    })?;
    if normalized_github_repository(&remote_url).as_deref()
        != Some(&request.repository.to_ascii_lowercase())
    {
        return Err(WorktreeError::Git(format!(
            "remote {} does not identify GitHub repository {}",
            request.remote, request.repository
        )));
    }
    if request.target.exists() {
        return verify_existing(request, &source);
    }
    if let Some(parent) = request.target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|source| WorktreeError::Io { path: parent.to_path_buf(), source })?;
    }
    tokio::task::block_in_place(|| {
        let repo = Repository::open(&source)?;
        let mut remote = repo.find_remote(request.remote)?;
        remote.fetch(
            &[&format!("refs/heads/{0}:refs/remotes/{1}/{0}", request.head_ref, request.remote)],
            None,
            None,
        )?;
        let reference = repo
            .find_reference(&format!("refs/remotes/{}/{}", request.remote, request.head_ref))
            .map_err(|error| {
                WorktreeError::Git(format!(
                    "fetched ref refs/remotes/{}/{} not found: {error}",
                    request.remote, request.head_ref
                ))
            })?;
        let mut options = WorktreeAddOptions::new();
        options.reference(Some(&reference));
        let _worktree = repo
            .worktree(request.local_branch, request.target, Some(&options))
            .map_err(|error| {
                WorktreeError::Git(format!(
                    "cannot add worktree at {}: {error}",
                    request.target.display()
                ))
            })?;
        Ok::<(), WorktreeError>(())
    })?;
    verify_existing(request, &source)
}

fn verify_existing(
    request: &WorktreeRequest<'_>,
    source: &Path,
) -> Result<ProvisionedWorktree, WorktreeError> {
    let expected = request.target.canonicalize().map_err(|source_err| WorktreeError::Io {
        path: request.target.to_path_buf(),
        source: source_err,
    })?;
    let target_root = tokio::task::block_in_place(|| canonical_repository(request.target))
        .map_err(|_| WorktreeError::TargetConflict(request.target.to_path_buf()))?;
    if target_root != expected {
        return Err(WorktreeError::TargetConflict(request.target.to_path_buf()));
    }
    let branch = tokio::task::block_in_place(|| {
        let repo = Repository::open(request.target)?;
        let head = repo.head()?;
        Ok::<String, WorktreeError>(head.shorthand()?.to_owned())
    })?;
    if branch != request.local_branch {
        return Err(WorktreeError::TargetConflict(request.target.to_path_buf()));
    }
    Ok(ProvisionedWorktree {
        source: source.to_path_buf(),
        path: expected,
        head_ref: request.head_ref.to_owned(),
        local_branch: request.local_branch.to_owned(),
    })
}

fn canonical_repository(path: &Path) -> Result<PathBuf, WorktreeError> {
    if !path.is_dir() {
        return Err(WorktreeError::NotRepository(path.to_path_buf()));
    }
    Repository::discover(path)
        .map(|repo| {
            repo.workdir().map_or_else(|| repo.path().to_path_buf(), Path::to_path_buf)
        })
        .map_err(|error| {
            if error.class() == ErrorClass::Repository && error.code() == ErrorCode::NotFound {
                WorktreeError::NotRepository(path.to_path_buf())
            } else {
                WorktreeError::Git(error.to_string())
            }
        })
}

fn normalized_github_repository(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))
        .or_else(|| remote.strip_prefix("git@github.com:"))?;
    let repository = path.strip_suffix(".git").unwrap_or(path).trim_matches('/');
    (repository.split('/').count() == 2).then(|| repository.to_ascii_lowercase())
}
