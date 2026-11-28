ALTER TABLE platform_grants ADD COLUMN authority_cursor INTEGER CHECK(authority_cursor >= 0);
CREATE INDEX local_event_authority ON local_events(item_number, event_id)
    WHERE kind IN ('assign','unassign','lifecycle') OR detail='run_policy';

-- A writer epoch consumes the canonical object history. Object transitions
-- never rewrite grant rows.
CREATE VIEW local_writer_authority AS
SELECT i.number, i.state, i.desired_profile,
       (WITH RECURSIVE ancestors(number) AS (
           SELECT i.number
           UNION SELECT issue_number FROM local_links WHERE pr_number=i.number
           UNION SELECT p.parent_number FROM local_issue_parents p
                 JOIN ancestors a ON p.child_number=a.number
        ) SELECT root_number FROM ancestors a JOIN run_root_policies p ON p.root_number=a.number)
        AS root_number,
       COALESCE((SELECT MAX(e.event_id) FROM local_events e
                 WHERE e.item_number=i.number AND e.kind IN ('assign','unassign','lifecycle')), 0)
        AS item_cursor
FROM local_items i;

CREATE VIEW local_run_writer_authority AS
SELECT p.root_number, p.client_id, root.state,
       COALESCE((SELECT MAX(e.event_id) FROM local_events e WHERE e.item_number=p.root_number
                 AND (e.kind='lifecycle' OR e.detail='run_policy')), 0) AS root_cursor
FROM run_root_policies p JOIN local_items root ON root.number=p.root_number;

CREATE VIEW effective_platform_grants AS
SELECT g.*, COALESCE(
       g.lifecycle='active' AND g.execution_id IS NOT NULL
       AND g.authority_cursor >= a.item_cursor AND a.state='open' AND a.desired_profile=g.actor
       AND NOT EXISTS(SELECT 1 FROM execution_fences f
                      WHERE f.client_id=g.client_id AND f.execution_id=g.execution_id)
       AND (a.root_number IS NULL OR
            p.client_id=g.client_id AND p.state='open' AND g.authority_cursor >= p.root_cursor),
       0) AS active
FROM platform_grants g
JOIN local_writer_authority a ON a.number=g.item_number
LEFT JOIN local_run_writer_authority p ON p.root_number=a.root_number;
