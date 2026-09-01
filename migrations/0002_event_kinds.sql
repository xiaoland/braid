-- Platform-neutral internal event model: the events ledger stores the typed
-- internal EventKind (assign/unassign/mention/wake/invalidate/lifecycle/
-- origin_echo/noop) plus an optional semantic detail, never platform event
-- names. Consumers branch on kind/detail only.

ALTER TABLE events RENAME COLUMN classification TO kind;
ALTER TABLE events ADD COLUMN detail TEXT;
