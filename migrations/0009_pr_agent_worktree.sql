ALTER TABLE worktrees ADD COLUMN source_path TEXT;
ALTER TABLE worktrees ADD COLUMN head_ref TEXT;
ALTER TABLE worktrees ADD COLUMN local_branch TEXT;

CREATE INDEX worktrees_repository_lifecycle
ON worktrees(repository_node_id, lifecycle);
