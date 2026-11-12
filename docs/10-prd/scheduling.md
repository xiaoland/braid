## Scheduling, Invalidation, and Reactions

Quiet time is debounce, not readiness. The default is 30 seconds and the
default count threshold is eight Wake Events; either condition releases the
batch. A repository MAINTAIN/ADMIN actor's exact visible `@braid` bypasses both.
Code, quotes, HTML comments, Braid-origin content, and less-privileged actors do
not create a trusted mention.

Replacing or removing a fact already present in a Work Item's Context is Hard
Invalidation. Braid fences the stale Context Revision immediately. If a turn is
active, it requests safe interruption, discards later stale Agent output, starts
a fresh physical Provider Session with current Context, and continues once
with the invalidation reference. If idle, it replaces Context without starting
a turn. Rapid Cross-surface edits to an open Associated Issue description wait
for debounce before interrupting a PR turn. Other Associated Issue changes mark
the dependency dirty and are incorporated before the next PR turn.

Every newly ingested external comment receives Braid's `eyes` reaction. Only a
Trusted Braid Mention has turn-lifecycle reactions on that same comment:

- `rocket` after the provider accepts the turn;
- `+1` after a normal terminal;
- `confused` after a confirmed unexpected terminal;
- back to `eyes` after safe invalidation supersession;
- `eyes` plus `rocket` while the result remains unknown.

Ordinary debounced turns never receive active or terminal reactions. Their
operational failures use one mutable Operational Status Comment instead, so
the normal collaboration model does not look like a request/response pipeline.

Explicit Issue→existing PR handoff is one internal ordinary Wake reference, not
a mention, activation, steer or interrupt. It merges pending/runnable batches
under the usual quiet/count policy. A running target receives it in a subsequent
batch, never a parallel turn. Before that batch's provider input, Braid freshly
materializes complete canonical PR Context including directly associated OPEN
Issue bodies/discussion. Read failures defer the unsent claim; no cached/partial
Context is substituted, and no 60-second reconciliation poll is required.
OriginEcho suppression and trusted mention rules remain unchanged: an App
comment containing `@braid` does not implicitly hand off anything.
A batch that includes a handoff cannot be forwarded as an in-turn urgent steer,
even if it also contains a trusted mention: that combined input waits for the
next safe turn boundary and retains its existing runnable/urgent status.
