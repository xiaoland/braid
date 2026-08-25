## Claims and Evidence

| Product claim | Observable success | Acceptance evidence |
| --- | --- | --- |
| GitHub is the Agent's durable working memory. | A fresh provider session receives the complete current Issue or PR Context and behaves according to edits, folds, deletion, and metadata changes rather than stale history. | Captured rendered Context, GitHub lifecycle state, and the Agent's subsequent public behavior. |
| Discussion and implementation have distinct roles. | Issue Agents discuss and maintain design; PR Implementation Agents work from the PR plus every directly Associated Issue. | Distinct Profiles/sessions, native associations, dedicated PR worktree, comments, and diff. |
| Collaboration is asynchronous by default. | Ordinary activity is debounced and coalesced; only a trusted visible `@braid` gives request-like turn reactions and bypasses debounce. | Event/reaction timestamps and absence of terminal reactions on ordinary batches. |
| GitHub edits can invalidate stale provider context. | Replacement/removal of already-materialized facts fences the old Context Revision and safely replaces the physical session without reviving folded content. | Provider-session generation, canonical Context, interruption, and subsequent Agent behavior. |
| Agents remain autonomous GitHub participants. | Agents publish concise comments and maintain descriptions/metadata themselves. `braid gh` provides stable App-authored writes without preventing normal `gh`, `git`, or shell use. | GitHub authorship, comment/body history, and public command results. |
| The runtime is diagnosable and distributable. | A packaged macOS arm64 binary runs the real flow with durable migrations and sampled full-fidelity OpenTelemetry. | Clean-install campaign, schema ledger, OTLP evidence, and restart/upgrade observations. |
