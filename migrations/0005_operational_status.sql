ALTER TABLE status_comments ADD COLUMN remote_comment_database_id TEXT;

CREATE UNIQUE INDEX status_comments_remote_node
ON status_comments(remote_comment_node_id)
WHERE remote_comment_node_id IS NOT NULL;
