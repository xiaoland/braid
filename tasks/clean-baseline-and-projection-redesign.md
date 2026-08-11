# Clean Baseline and Projection Redesign

- **Objective**: Retire the failed Issue 20 / PR 21 dogfood run without losing its product and technical evidence, then define—step by step with the Human—the exact Codex app-server items that the Wrapper may project as one GitHub turn mirror before any new implementation or external run.
- **Guardrails**: Preserve the product boundary already agreed: one Issue maps to one provider-owned Agent thread; GitHub is the collaboration authority; the Agent alone interprets messages and manages `gh`, worktrees, branches, commits, and PR actions; the Wrapper only synchronizes canonical GitHub refs, schedules turns, steers with opaque refs, and projects explicitly approved Agent-facing items. Keep all PoC files inside `agent-handoff/`; keep collaboration instructions project-scoped; do not modify the primary worktree, this bootstrap worktree, or user-scope `AGENTS.md`. Do not restart the Wrapper, tunnel, webhook, or external dogfood until a replacement projection contract and black-box acceptance plan are approved. Never publish raw CoT, raw tool arguments, raw tool output, environment data, credentials, or arbitrary protocol events merely because app-server emitted them.
- **Verification**: Baseline cleanup is complete: PR 21 is closed and Issue 20 is deleted with its comments; the Issue-20 provider and three protocol-probe Codex tasks are archived; the dedicated Issue-20 and mirror-backpressure worktrees, local branches, remote branches, and remote-tracking refs are absent; obsolete repository webhook `664148764` is absent; no related Worker, Quick Tunnel, or PoC stdio app-server process is running; the current bootstrap worktree is intact except for this packet. Before a new dogfood run, an approved item-by-item projection table and a clean-ancestry branch check must exist, and the public PR diff must contain only `agent-handoff/` paths.
- **Current Truth**: The first real dogfood run is rejected as acceptance evidence and its execution surfaces are retired. It produced four Agent turns but 196 current Issue comments, including 192 Wrapper marker comments. Codex app-server emitted structured JSONL notifications, not base64. The Wrapper did not persist a byte-exact raw stdout log; it accumulated selected notifications in memory, encoded the whole cumulative snapshot as base64, split it into comments, and rewrote all shards every five seconds. The selection incorrectly treated protocol deltas and raw execution evidence as publishable messages: it retained `item/commandExecution/outputDelta` plus terminal `commandExecution.aggregatedOutput`, commands, cwd, file changes, and tool arguments/results. In the fourth turn, `gh issue view 20 --json ...comments...` returned a 1,048,605-byte `aggregatedOutput` containing existing mirror comments; the Wrapper mirrored that output back to the same Issue, creating a recursive amplification loop. The reconstructed fourth-turn prefix alone contains 2,530 projected items and 3,457,740 decoded JSON bytes. Raw reasoning deltas were excluded, while provider-labelled reasoning summaries were included. The failure therefore does not yet decide whether a bounded hidden turn projection is valuable; it proves that app-server transport events and raw tool I/O are not themselves publishable turn messages, and that recursive GitHub content must not re-enter its own projection. PR 21 is also invalid: its five PoC commits each touched only `agent-handoff/`, but the branch was created on a stack containing 17 unrelated commits and opened against GitHub `main`, so the public diff showed 257 files, including 206 outside the allowed directory. A separate worktree did not protect branch ancestry, and no pre-push path gate caught the error. The replacement projection starts from three semantic classes only: assistant message, provider-labelled reasoning summary, and tool-call summary. Wrapper must parse the app-server event stream through a reducer and reconstruct an ordered, turn-scoped projection message history; protocol events are inputs to that reducer, not history entries. This history exists only to render the GitHub mirror and never reconstructs, replaces, or becomes authority for the provider thread. Fixed five-to-ten-second publication is rejected. Intermediate publication uses a dirty logical-message count threshold OR a long maximum dirty-age threshold, whichever occurs first; the time threshold is a liveness fallback and should be as large as product feedback permits, not a target cadence. A terminal turn flushes immediately, and an unchanged projection never causes an edit. Message count refers to newly completed publishable logical messages, never delta-frame count. The pinned-schema reducer lifecycle, stable identity, coalescing, phase handling, field allowlists, mechanical tool-summary source, and bounded restart checkpoint are now specified in the supporting contract. Threshold values, byte ceilings, final-answer overflow behavior, and optional provider-labelled plugin identifier display remain calibration decisions.
- **Next Step**: The Human authorized the bounded [minimal mirror smoke](minimal-mirror-smoke.md). Use that packet as the active mutation and evidence gate; do not resume the rejected full Issue-to-PR campaign from this packet.

## Retained Product Decisions

- Every GitHub comment remains a message; receipt never implies a command. An exact trusted visible `@agent` is only an urgent scheduling hint that bypasses the quiet window.
- Ordinary events settle mechanically. Events arriving during an active turn are delivered as opaque safe-point steer refs; the Wrapper never interprets them or forces a semantic interrupt.
- Issue is the product/design/acceptance source of truth; a natively associated PR is a candidate implementation surface. The Wrapper knows associated PR surfaces only to route comments, reviews, edits, deletions, minimization, and resolution into the Issue's one Agent thread.
- Provider/app-server exclusively owns thread history, persistence, compaction, and resume. The Wrapper stores only an opaque Issue-to-thread address and transport/mirror delivery facts; it does not create reset-points or replacement threads.
- A mixed-surface turn has one canonical mirror. Other participating surfaces receive at most one bounded Wrapper FYI link; the Wrapper does not copy prose or infer design meaning.
- The Agent must use a dedicated worktree and a branch rooted at the intended remote base. Before push/PR creation, verification must prove both ancestry and path scope; `git diff --name-only <remote-base>...HEAD` may contain only `agent-handoff/` paths.

## Rejected Publication Assumptions

- “Protocol-visible” does not mean “publishable.” Delta frames, lifecycle events, tool arguments, raw output, completed aggregated output, and file-change payloads cannot be mirrored wholesale.
- A fixed periodic timer is not the publication model. Time is only a long maximum-wait fallback for a dirty projection; logical-message accumulation is the primary batching signal, terminal is immediate, and no change means no edit.
- HTML comments are presentation only, not confidentiality or notification suppression. Base64 is encoding, not protection.
- One logical turn does not justify one GitHub comment per shard. Sharding is only a last resort for approved semantic content that exceeds a bounded body budget.
- Cumulative snapshot rewrites cannot depend on GitHub content that the Agent may query back into the same turn.

## Projection Topology Under Discussion

```text
app-server JSONL events
  -> typed event parser
  -> per-item lifecycle reducer keyed by thread/turn/item identity
  -> ordered turn-scoped projection messages
       assistant message | reasoning summary | tool-call summary
  -> dirty logical-message counter + oldest-dirty time
  -> GitHub mirror renderer/outbox
```

- `started`, delta, and `completed` events update one stable logical item; they do not each append a public message.
- A publication success resets the dirty count and oldest-dirty time without deleting the turn-scoped message history needed for the next complete projection.
- Count-threshold publication, maximum-dirty-age publication, and terminal publication are triggers over the same projection snapshot. They must not create parallel comments or migrate the canonical target.
- The sanitized projection history is minimally checkpointed until the terminal mirror is acknowledged, then deleted. Provider-owned current-turn snapshots reconcile it after Wrapper restart. This bounded mirror recovery does not grant the Wrapper ownership of thread history or compaction.

The reducer analysis is now captured in
[projection-reducer-contract.md](projection-reducer-contract.md). It resolves
stable identity, assistant phase handling, reasoning-summary reconstruction,
mechanical tool summaries, count/time triggers, and a bounded sanitized
checkpoint for mirror-only recovery. Remaining values are calibration rather
than authority or topology decisions.

## Cleanup Scope

- GitHub: delete `xiaoland/svc#20` and its comments; close Draft PR `xiaoland/svc#21`; delete inactive repository webhook `664148764`; remove remote branches `feat/issue-20-provider-reconnect` and `hotfix/turn-mirror-backpressure` after the objects close.
- Codex: archive provider task `019feeac-2de4-7322-b414-857b3a20576f` and PoC protocol-probe tasks `019fec59-8762-72c2-b632-d484a1497d84`, `019fec58-8b64-71a2-8b0c-8d1454958f0b`, and `019fec3c-d364-7b01-9b88-feb3264e441b`.
- Git: remove only `/Volumes/WorkSSD/Development/.worktrees/issue-20/svc` and `/Volumes/WorkSSD/Development/.worktrees/turn-mirror-backpressure/svc`, then remove their local branches. The 22 dirty entries in the Issue-20 worktree were verified byte-for-byte against remote commit `33f1e0b9b5c6e0cf0c0104617d8059409ec8df8a`; no mismatch exists.
- Preserve the primary worktree, current `/Volumes/WorkSSD/Development/.worktrees/5cc2/svc` bootstrap worktree and branch, all unrelated worktrees/tasks, provider rollout evidence, and the stopped local runtime directory until the redesign decides whether further forensic evidence is needed.
