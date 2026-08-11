# Product Truth

Braid turns a GitHub Issue and one local Coding Agent thread into one durable,
Human-visible collaboration loop. GitHub is where the team discusses and
reviews the work; the Coding Agent remains a local tool with its own provider
thread; Braid carries references and projects the Agent's turn without becoming
the product or engineering decision-maker.

## Product Objects

- The bound **Issue** is the product/design/acceptance source of truth.
- The bound **Agent thread** is the Coding Agent's provider-owned working
  context. One Issue maps to one thread in the current product.
- A natively associated **Draft PR** is the candidate implementation surface.
  The Agent creates, links, updates, and eventually readies it; Braid only knows
  the association needed to route collaboration.
- A GitHub **comment** is always a message. Receiving one never makes it a
  command. Edits, deletion, minimization, reviews, and resolution remain
  lifecycle changes to canonical GitHub objects.
- A Braid **turn mirror** is a Human-readable projection of one Agent turn onto
  one canonical GitHub comment. It is not the provider transcript.

## Collaboration Promise

1. Ordinary Issue or associated-PR activity settles before waking the Agent so
   active discussion does not produce a turn per comment.
2. An exact visible trusted `@agent` skips the quiet window. It changes timing,
   not authority or meaning.
3. Braid tells the existing thread which canonical GitHub objects changed. The
   Agent uses `gh` to observe current state and independently decides whether to
   discuss, wait, plan, implement, pause, or replan.
4. Once discussion is sufficiently mature, the Agent works in its dedicated
   worktree, creates a Draft PR, and establishes GitHub's native Issue link.
5. Issue and linked-PR discussion continue steering the same Agent thread. The
   Issue keeps material design/acceptance changes; the PR keeps candidate diff,
   verification, and review response.
6. Humans can follow each turn as readable assistant messages, provider-labelled
   reasoning summaries, and bounded tool activity. The terminal response replaces
   the processing state in the same canonical comment.

When one turn involves both Issue and PR surfaces, Braid freezes one canonical
reply surface mechanically and places one short link-only FYI on every other
participating surface. It never copies the response across surfaces or reads
the prose to decide where it belongs.

## Authority and Non-goals

- Braid does not interpret Human intent, judge readiness or acceptance, manage
  Agent context, create replacement provider threads, create PRs, choose
  branches, manage worktrees, merge, or close Issues. It may ask app-server to
  start the single initial thread when establishing a new binding.
- Codex app-server owns thread history, compaction, turn execution, and resume.
  GitHub owns Issue/PR/comment/review state. The Agent owns semantic decisions
  and its authorized `gh` actions. Braid owns only transport, scheduling, and
  the remote turn projection.
- The mirror never contains raw chain-of-thought, arbitrary protocol envelopes,
  hidden debug payloads, or provider IDs. Its bounded tool evidence is intended
  for Human observation, not confidentiality; general free-form secret
  redaction remains outside the current prototype.
- Multiple candidate PRs, multiple Agent threads per Issue, raw repository push
  routing, merge automation, and production-grade hosted ingress are outside
  the current boundary.

## Maturity and Acceptance

Braid is an unaccepted prototype. Local checks, protocol probes, health, and
historical smokes can diagnose it but cannot prove the product promise. Product
acceptance requires the real Issue-to-Draft-PR campaign in
[`acceptance.md`](acceptance.md).
