## Purpose and Pressure

Long Coding Agent sessions accumulate tool output, superseded requirements, and
stale operational facts. Provider compaction helps with token pressure but does
not know which GitHub edits, hidden comments, resolved review threads, or
current metadata supersede that history. Traditional chat also leaves settled
design and implementation state scattered across a private transcript.

Braid makes the collaboration surface itself the compacted memory:

- an Issue description carries the current design;
- a PR body carries the current implementation intent and state;
- visible comments retain discussion and Agent messages;
- folded or resolved discussion keeps identity and lifecycle metadata but not
  body content;
- current GitHub metadata and relationships remain available without expanding
  every related object recursively.

An Agent may therefore replace a stale provider session without losing the
team's current understanding. Humans can correct that memory with ordinary
GitHub edits instead of manipulating a private Agent transcript.
