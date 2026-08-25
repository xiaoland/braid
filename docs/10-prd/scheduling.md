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
