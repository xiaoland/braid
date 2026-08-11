# Bootstrap Implementation

- **Objective**: Build and pin the smallest trustworthy bootstrap Wrapper that can self-host the real GitHub Issue-to-Draft-PR workflow, including native-associated-PR routing and mixed-surface publication, then prove the candidate through fresh-install end-to-end black-box acceptance.
- **Guardrails**: Keep every product file inside the independent Braid repository. The product is named Braid. Wrapper remains transport-only and never creates/interprets PRs, manages worktrees, or persists Agent thread data. GitHub and app-server remain their own authorities. Bootstrap `B`, the Human primary worktree, and candidate `C` use distinct worktrees, branches, runtime directories, and process identities. Only one process may own a binding. The future source Issue must visibly tell the Agent to use only its provisioned candidate worktree and never modify the bootstrap or primary worktree. External GitHub App, webhook, Issue, Cloudflare, and repository mutations require a separately announced exact mutation gate. Collaboration instructions remain project-scoped and must not modify user-scope behavior.
- **Verification**: The implementation boundary is reviewable before mutation; every slice has an externally observable verification path; provider and GitHub assumptions are probed against their real public protocols; protected worktrees remain byte/status stable; pinned `B` completes the self-hosted Issue-to-Draft-PR handoff; clean `C` installations pass the approved black-box suite three consecutive times; no unit or integration result substitutes for end-to-end acceptance.
- **Current Truth**: The isolated branch `feat/github-collaboration-bootstrap` contains Braid under this directory only. The current local suite has 101 passing guards. Implemented boundaries include: strict secret references and an exact provider environment; run-scoped read-only or workspace-write provider sandbox; the probed Codex JSONL transport; SQLite schema v1 with owner fencing, bindings, exact Issue/current-native-PR surface routing, delivery dedupe, scheduler state, mirror/outbox state; raw-byte GitHub HMAC ingress; bounded canonical reconciliation for Issue, PR conversation, review, review-comment, and review-thread lifecycle; exact visible lowercase `@agent` with canonical actor permission; quiet/urgent single-turn scheduling; Wrapper-origin provider application refs; and a turn-scoped semantic reducer admitting assistant messages, provider-labelled reasoning summaries, and schema-mapped bounded tool calls. One visible Markdown comment replaces hidden escaped JSON: tool calls use Human summaries and folded call/result evidence, final response is promoted, raw CoT/debug IDs/markers are absent, completed-message-count OR maximum-dirty-age controls intermediate edits, and terminal state edits the same comment. Marker-free create recovery, Human-edit conflict, mixed-surface FYI reference publication, protocol-pinned app-server startup/resume, loopback health, GitHub reconciliation, owner-lease renewal, and supervised free Quick Tunnel remain implemented. REST/webhook `app[bot]` and GraphQL `app` actor spellings normalize to one self identity. The packaged probe remains pinned to Codex app-server 0.147.0-alpha.6.5 with stable-schema SHA-256 `7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb` and experimental-schema SHA-256 `a14d4878fe7b8cdd31059dbca11d7167d8cfd06effa2f7991b5364439063a5c8`. Issue 23 is historical reducer/idempotency evidence. The fresh visible-projection smoke on Issue 24 passed with one turn, one stable comment ID, Human-readable tool details, bounded large output, no hidden payload, and no self-echo turn after reconciliation; temporary webhook/tunnel/runtime surfaces were removed afterward. Full Issue-to-PR acceptance remains separate.
- **Next Step**: Under the approved external mutation gate, provision the dedicated GitHub App and identities, disposable real Issue, candidate worktree/branch, and free Quick Tunnel. The next verification is the real black-box Issue-to-Draft-PR campaign; local guards do not constitute product acceptance.
- **Extraction correction (2026-08-11)**: Braid is now an independent repository; the historical bootstrap branch and SVC-subdirectory address in Current Truth are retained only as implementation provenance.

## Topology and Ownership

```text
GitHub App webhook
  -> Cloudflare Quick Tunnel (TLS/byte forwarding only)
  -> loopback ingress (raw HMAC verification)
  -> durable transport inbox / scheduler
  -> opaque Codex app-server thread address
  -> Agent uses gh against GitHub canonical state

app-server live turn notifications
  -> allowlisted mirror projection / outbox
  -> one Issue comment per bootstrap turn
```

- `github_transport`: GitHub App authentication, webhook verification, Issue canonical references, REST/GraphQL reads, mirror comment create/edit, and webhook configuration. It owns no task meaning.
- `binding_store`: one Issue node ID to one opaque provider thread address, instruction digest, transport lifecycle, event cursor, owner lease, and schema migration. It stores no transcript, worktree, branch, PR, readiness, or acceptance state.
- `scheduler`: pending refs, quiet generation/deadline, trusted exact-mention hint, one active turn, safe steer fallback, and coalescing. It never interprets prose.
- `agent_adapter`: typed JSON-RPC initialization, thread start/resume, turn start/steer/terminal notifications, live item observation, backpressure, and transport-unknown handling.
- `turn_mirror`: one logical visible Markdown comment per turn, completed-message-count OR maximum-dirty-age edits, explicit terminal projection, marker-free outbox reconciliation, and self-echo suppression.
- `operator_cli`: configuration validation, provider probe, and one-binding `serve`; the process lifecycle is the bootstrap bind/pause/handoff control, while bounded runtime status is exposed on loopback health. It does not leak secrets or thread content.

## Minimal Durable State

Use one process-owned SQLite database with explicit schema versioning and transactions:

- `bindings`: binding ID, repository/Issue node references, opaque provider thread address, Agent/Wrapper identities, trusted permission threshold, instruction digest, lifecycle, and lease generation.
- `events`: GitHub delivery GUID, event/action, object and surface refs, version/digest, observed time, urgent bit, and pending/delivered/superseded state. Canonical comment bodies are fetched from GitHub rather than treated as local authority.
- `schedulers`: binding generation, quiet deadline, urgent generation, active provider turn handle, and transport status.
- `mirrors`: provider turn ID, Issue target, remote comment IDs/chunks, terminal state, revision/body digest, and ownership/conflict state.
- `outbox`: deterministic operation key, remote target, intended digest, pending/sending/uncertain/acked state, attempts, and remote ID.
- `sync_cursors`: repository/binding reconciliation watermark and last successful canonical scan.

Bootstrap schema is deliberately additive so candidate `C` can copy and migrate a stopped `B` data directory without mutating the preserved rollback source.

## Execution Slices

1. **Pin protocols before architecture**
   - Generate and digest the installed app-server v2 schema.
   - Run a real local probe for initialize, thread start/resume, turn start, same-turn steer, non-steerable rejection, completed/interrupted/failed, and disconnect-without-terminal.
   - Record exactly which protocol item types may enter the mirror; unknown types fail closed.
   - Verify GitHub native linked-PR GraphQL fields and the dedicated App permissions needed by bootstrap and later candidate.

2. **Create the isolated runtime skeleton**
   - Add a child `pyproject.toml`, lockfile, package, CLI, and tests without changing root PDM, lock, CI, or imports.
   - Use Python 3.12 with PDM, `aiohttp` for the loopback server and GitHub HTTP client, Pydantic for strict configuration, built-in `sqlite3` behind one async owner, and PyJWT with its cryptographic backend for GitHub App authentication. Do not implement JWT signing or HTTP parsing locally.
   - Store secrets only by environment/file reference. Spawn app-server with an explicit environment allowlist that excludes the Wrapper App private key and webhook secret.

3. **Implement transport authority and persistence**
   - Add schema migration, single-owner lock/lease, atomic event ingestion, delivery GUID deduplication, pending scheduling, and outbox uncertainty.
   - Acknowledge GitHub only after raw HMAC verification and durable normalized-envelope storage; never wait for Agent work inside the webhook request.
   - Use stable node IDs and canonical URLs; numbers are display/routing aids only.

4. **Implement Issue-only GitHub ingress and reconciliation**
   - Handle bound Issue title/body/state and Issue-comment create/edit/delete plus reconciliation-observed minimization.
   - Reconcile the latest canonical state on startup and on the accelerated acceptance interval; deletion becomes a tombstone reference rather than an invented empty message.
   - Resolve actor permission mechanically. `triage|write|maintain|admin` may make an exact visible `@agent` urgent; unknown/lower permission remains an ordinary attributed message.

5. **Bind the real provider without owning it**
   - `serve` validates or creates one active binding per Issue and accepts a pre-provisioned, dedicated Agent worktree as the provider cwd.
   - Start or resume exactly one opaque thread and inject only dynamic Issue/repository/Wrapper addresses plus the durable project-scope collaboration contract.
   - Deliver event envelopes as Wrapper-origin refs; the Agent uses `gh` for canonical content and actions.
   - Start the real app-server over stdio as a supervised provider subprocess only after the protocol probe succeeds. Use `approvalPolicy=never` because Issue binding is the standing mandate, but constrain each turn to the dedicated candidate cwd with `workspaceWrite`, network enabled for `gh`, and only the exact additional writable roots required by that worktree's Git administration.
   - Provider unavailable or status-unknown pauses delivery without creating a replacement thread or inventing an Agent terminal result.

6. **Implement quiet scheduling and urgent steering**
   - Ordinary events reset a persisted 30-second acceptance quiet deadline; restart restarts a full quiet period rather than inferring silence from old timestamps.
   - Trusted exact visible `@agent` bypasses quiet, coalesces duplicates, and starts one turn from idle/pending.
   - Active input attempts same-turn steer. Non-steerable rejection preserves urgent/pending refs for the first safe terminal boundary; no parallel turn or forced interrupt.

7. **Implement the Issue turn mirror**
   - Create one stable logical comment when a turn becomes active; edit only when the body digest changes.
   - Keep the visible active text bounded. Render allowlisted protocol items with safe Markdown fences and HTML-escaped `<summary>` labels so payload content cannot escape its presentation boundary.
   - Publish only explicit `final_answer` on completed. Distinguish completed-without-final, interrupted, failed, and transport-unknown; none implies task success/failure.
   - Reconcile known comments by remote ID. Recover an uncertain create only from a unique target/Wrapper-author/body-digest/time-window match; otherwise fail closed. Human edit/delete creates conflict/lifecycle evidence; do not silently overwrite or resurrect it.

8. **Add zero-cost public ingress and operator health**
   - Start `wrangler tunnel quick-start` against loopback, capture and verify the temporary HTTPS URL, update the dedicated test GitHub App webhook through an App JWT, and validate GitHub ping/signature.
   - Expose bounded status for binding, ingress, tunnel, scheduler, provider connection, and mirror outbox. Do not expose app-server or admin controls through the tunnel.
   - Tunnel loss is transport unavailable; failed webhook events converge through GraphQL reconciliation because GitHub does not automatically redeliver them.

9. **Prove and pin bootstrap `B`**
   - Run focused developer guards, then a real Issue-only black-box smoke through GitHub -> Tunnel -> Wrapper -> app-server -> GitHub.
   - Record artifact/source/config/schema digests, runtime directory, binding identity, protected worktree HEAD/status, and app-server schema/version.
   - Freeze `B`; the self-hosted Issue may not hot-edit its source or runtime environment.

10. **Self-host the real source Issue and candidate `C`**
    - Operator provisions a new branch/worktree exclusively for the Issue and binds the Agent there; Wrapper never creates or chooses it.
    - The first Human Issue comment must include the worktree safety message below.
    - Two Humans discuss the real native-associated PR routing and mixed-surface publication feature. The Agent decides readiness and creates the linked Draft PR without a start command.
    - When candidate `C` is locally viable, stop `B`, preserve its data directory, copy it to a distinct `C` runtime directory, and start `C` exclusively on the same opaque binding. Rollback restarts untouched `B`; missed GitHub state is reconciled.
    - PR review must enter through `C`, steer the same thread, update the candidate, and preserve Issue design authority.

11. **Accept candidate independently**
    - Install `C` from a clean artifact/worktree with a new runtime directory and a fresh Issue/thread binding in a private mirror.
    - Run all approved black-box journeys three consecutive times without corrective intervention.
    - Compare protected primary and bootstrap worktree HEAD/status snapshots before and after; neither may change.
    - Only after both source-PR dogfood and clean-candidate evidence pass may the Agent publish verification and make the Draft PR Ready-for-review.

## Required Source Issue Comment

The real source Issue must contain a Human comment with this invariant before implementation begins:

> 本 Issue/PR 的实现必须只在为它单独创建并提供给 Agent 的 Git worktree 与 branch 中进行。不得修改正在运行 bootstrap Worker 的 worktree，也不得修改 repository 的 primary worktree；任何已有 dirty changes 都属于 Human。开始修改前请核对当前 workspace、branch、`git worktree list` 与 `git status`，发现身份或边界不一致时停止并在 Issue 中说明。对外只报告 branch/worktree identity，不公开本机绝对路径。

This is a normal Human message and visible safety constraint, not a Wrapper command. The same invariant also belongs in the durable project-scope collaboration instructions, while exact branch/worktree identities remain dynamic Issue context.

## Pre-mortem and Early Fault Removal

1. **Wrong worktree is mutated**: pre-provision candidate cwd, reject protected paths at bind, snapshot protected HEAD/status, include the visible Issue constraint, and verify before/after.
2. **Bootstrap and candidate both own one binding**: use an exclusive owner lock plus generation; handoff requires stopped-owner evidence before the next process starts.
3. **Candidate migration destroys rollback**: never migrate `B` in place; copy the stopped data directory and keep schema changes additive.
4. **Quick Tunnel URL changes or dies**: verify tunnel health before webhook update, retain the previous App configuration for diagnosis, and rely on canonical reconciliation rather than assumed GitHub retry.
5. **Webhook signature or acknowledgement is wrong**: test raw-byte HMAC, reject before parsing, durably enqueue before a fast response, and exercise GitHub ping/redelivery.
6. **Webhook delay is mistaken for settling**: start/reset quiet from local canonical observation, not GitHub timestamps; keep webhook, quiet, and reconciliation clocks separately observable.
7. **`@agent` self-triggers or is abused**: parse only exact visible Human text, check canonical actor permission, suppress Wrapper/Agent origins, and deduplicate comment version.
8. **Provider schema or steering differs from assumptions**: gate architecture on the real probe and pin its schema digest; unknown items/statuses fail closed and remain visible as transport unknown.
9. **Wrapper secrets leak into Agent/tool output**: do not inherit Wrapper credentials into app-server; use separate least-privilege Agent `gh` credentials and never put secrets in prompts/comments.
10. **Mirror creates duplicates after timeout/crash**: deterministic local outbox keys, marker-free canonical evidence before recovery, digest-based no-op suppression, and explicit Human-edit conflict.
11. **Hidden content escapes into rendered Markdown**: canonical encoding, bounded allowlist, adversarial `-->`/Unicode/control-character probes, and rendered/raw GitHub comparison.
12. **Self-hosting is mistaken for candidate proof**: require a clean `C` artifact, fresh binding, private-mirror full-stack campaigns, and evidence independent of the source PR thread.
