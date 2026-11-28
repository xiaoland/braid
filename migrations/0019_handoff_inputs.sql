-- The accepted input carries all facts needed by the scheduler and Context.
ALTER TABLE events ADD COLUMN next_turn_only INTEGER NOT NULL DEFAULT 0
    CHECK (next_turn_only IN (0,1));
UPDATE events SET next_turn_only=1 WHERE kind='wake' AND detail='pr_handoff';

CREATE TABLE handoff_inputs (
    event_id TEXT PRIMARY KEY REFERENCES events(event_id),
    source_repository TEXT NOT NULL CHECK (length(source_repository)>0),
    source_number INTEGER NOT NULL CHECK (source_number>0),
    target_assignment_id TEXT NOT NULL REFERENCES assignments(assignment_id),
    target_generation INTEGER NOT NULL CHECK (target_generation>0)
) STRICT;

-- An old Wake without recoverable evidence remains next-turn-only and is
-- retired by the input consumer before any provider delivery.
INSERT INTO handoff_inputs
SELECT e.event_id,w.repository,source_item.number,target.assignment_id,target.generation
FROM events e
JOIN pr_handoffs h ON h.event_id=e.event_id
JOIN write_intents w ON w.intent_id=h.intent_id AND w.operation='pr_handoff'
JOIN assignments target
    ON target.assignment_id=h.target_assignment_id AND target.work_item_node_id=e.work_item_node_id
JOIN work_items target_item ON target_item.node_id=target.work_item_node_id AND target_item.kind='pr'
JOIN assignments source ON source.assignment_id=CASE WHEN json_valid(w.source_binding)
    THEN json_extract(w.source_binding,'$.assignment_id') END
JOIN work_items source_item ON source_item.node_id=source.work_item_node_id
    AND source_item.kind='issue' AND source_item.repository_node_id=target_item.repository_node_id
WHERE e.next_turn_only=1 AND length(w.repository)>0;

-- Before delivery the request owns the reserved target. After delivery the
-- receipt retains a reference to the accepted event.
CREATE TABLE pr_handoffs_v19 (
    intent_id TEXT PRIMARY KEY REFERENCES write_intents(intent_id),
    target_assignment_id TEXT REFERENCES assignments(assignment_id),
    event_id TEXT UNIQUE REFERENCES events(event_id),
    CHECK ((target_assignment_id IS NULL) <> (event_id IS NULL))
) STRICT;
INSERT INTO pr_handoffs_v19
SELECT intent_id,CASE WHEN event_id IS NULL THEN target_assignment_id END,event_id FROM pr_handoffs;
DROP TABLE pr_handoffs;
ALTER TABLE pr_handoffs_v19 RENAME TO pr_handoffs;
