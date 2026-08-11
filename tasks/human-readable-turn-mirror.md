# Human-readable Turn Mirror

- **Objective**: Give Braid one fully visible, Human-oriented Markdown comment per Agent turn. Preserve the semantic reducer and batching policy while rendering actual assistant-message and reasoning-summary text plus each tool call's descriptive `<summary>` and expandable call/result `<details>` content.
- **Guardrails**: The GitHub body contains no HTML comment, app-server item/turn/thread ID, revision, sequence, lifecycle enum dump, ownership marker, body digest, raw CoT, whole environment snapshot, credential-bearing structured field, or arbitrary protocol envelope. Tool call payload and result are now intentionally publishable Human evidence, but only through a schema-specific projection and explicit byte bounds; they are never admitted by serializing the entire protocol item. Wrapper still does not interpret task meaning or ask an LLM to rewrite tool activity. The explicit final assistant message must never be dropped. Human edits remain conflicts and are never silently overwritten. Keep all source changes inside the Braid repository; do not run another external smoke until the Markdown renderer, bounded tool evidence, and marker-free uncertainty contract pass locally.
- **Verification**: Golden Markdown snapshots cover active, completed, interrupted, failed, partial assistant, multi-part reasoning summary, every supported tool's descriptive summary, actual call payload, actual result/error, oldest-message omission, per-field and per-tool truncation, Markdown/fence injection, recursive GitHub-comment output, and total comment overflow. Negative assertions reject `<!--`, projection JSON records, item IDs, turn IDs, ownership markers, raw CoT, Wrapper-owned secrets, structured environment/credential fields, and unknown protocol fields. Publisher tests prove known-remote edit/no-op/conflict behavior and fail-closed uncertain-create recovery without relying on text markers. Real-Issue evidence must inspect both rendered Markdown and raw body size, show one stable comment ID, and prove a large or recursive tool result is visibly truncated rather than recursively mirrored.
- **Current Truth**: The project is now named Braid. The approved replacement is implemented locally: assistant commentary remains actual Markdown; provider-labelled reasoning renders under bold `Reasoning summary`; every supported tool item coalesces into one `<details>` block whose `<summary>` carries the friendly name, status, and bounded facts while its body contains schema-mapped call/result evidence in injection-safe fences. Command output deltas are provisional and the completed `aggregatedOutput` replaces them; file patches, MCP content/error, dynamic content, anonymous collab outcomes, web-search actions, image view, and image generation each have explicit pinned-schema mappings. Raw reasoning, process/thread/item IDs, opaque web result objects, MCP metadata/structured content, unknown protocol fields, binary content, and structured secret fields are excluded. Call and result limits are independently run-scoped at 8 KiB and 16 KiB by default, with a 256 KiB projection bound and 60 KiB comment bound; truncation keeps a bounded prefix/suffix with an explicit Braid notice. The renderer emits visible Markdown only, promotes the final assistant response, removes it from the chronological activity, omits oldest activity rather than sharding, and refuses to truncate an oversized final silently. Remote bodies contain no debug JSON or ownership marker. Publisher recovery now uses the unique conjunction of target, normalized Wrapper author, exact body digest, and bounded create-time window; zero or multiple matches remain uncertain and fail closed. The public product/CLI/client name is `braid`; the internal Python import path remains `github_agent_bridge` for this slice to avoid a mechanically enlarged diff. The first two Issue 24 attempts correctly preserved the no-command identity boundary but exposed an incomplete transport prompt: the Agent did not perform canonical observation. Their Wrapper comments were deleted and provider tasks archived. The prompt now explicitly requires `gh` observation before independent judgment. The clean retry then passed on [Issue 24](https://github.com/xiaoland/svc/issues/24): one provider turn produced one Wrapper comment (`5252609923`), six projection edits converged on the same remote ID, and the scheduler remained idle through a later reconciliation interval. The final body is 13,159 UTF-8 bytes with six Human-readable command `<details>` blocks, six Call/Result pairs, the deterministic `braid-smoke-line` fixture, and three visible Braid truncation notices. It begins with the final response and contains no HTML comment, ownership marker, or serialized turn/thread/item/body-digest field. No provider reasoning summary was emitted in this turn, so none was invented. GitHub accepted the create/edit webhook loop with `202`, self echoes created no new turn, the read-only provider changed no file outside the already-authorized `agent-handoff/` work, and the temporary repo webhook, tunnel, and Braid process were removed after verification. All three provider tasks used during setup/retry are archived. The complete child suite passes with 101 tests; `git diff --check`, `pdm lock --check`, and `pdm run braid --help` also pass.
- **Next Step**: The Human accepted the publication shape. Preserve this packet as completed slice evidence while Braid moves to its independent repository; the next product acceptance remains the separate full Issue-to-Draft-PR black-box campaign.

## Markdown Projection Shape

Active turn:

```markdown
> ⏳ **Agent 正在处理**

### Turn activity

**Agent**

我先读取当前 Issue，并核对工作区边界。

**Reasoning summary**

已确认任务只需要只读检查。

<details>
<summary>✅ <strong>Command</strong> completed — exit 0 · 855 ms</summary>

**Call**

```shell
gh issue view 23 --repo xiaoland/svc
```

**Result**

```text
title: [PoC smoke retry] Mirror exactly one Codex app-server turn
state: OPEN
```

</details>
```

Completed turn:

```markdown
## Final response

已完成只读核对，没有修改仓库。

---

### Turn activity

**Agent**

我先读取当前 Issue，并核对工作区边界。

**Reasoning summary**

已确认任务只需要只读检查。

<details>
<summary>✅ <strong>Command</strong> completed — exit 0 · 855 ms</summary>

**Call**

```shell
gh issue view 23 --repo xiaoland/svc
```

**Result**

```text
title: [PoC smoke retry] Mirror exactly one Codex app-server turn
state: OPEN
```

</details>
```

The `<summary>` label, status icon, field labels, fence language, and compact
detail ordering are renderer-owned presentation. Tool payload/result content is
copied from explicitly mapped app-server fields, subject only to the approved
redaction and bounded-truncation publication policy. Agent message and
reasoning-summary bodies remain provider-authored Markdown. The Wrapper may
normalize surrounding blank lines and choose a safe fence longer than any
backtick run in the content, but it must not paraphrase or classify task meaning.

## Tool Projection Shape

Every supported tool item reduces into four presentation fields:

1. `description`: a deterministic Human label derived from the schema-known
   tool kind/name and terminal state, not an LLM-authored semantic summary;
2. `call`: the actual schema-approved request payload, command, query, patch, or
   arguments that the Agent submitted;
3. `result`: the actual schema-approved output, response, error, or change
   result returned to the Agent;
4. `facts`: bounded mechanical facts used in `<summary>`, such as duration,
   exit code, success, read-only hint, or change count.

Started/delta/completed events update one stable tool message. Payload and
result deltas are accumulated into that message and the authoritative completed
snapshot replaces provisional fields when the protocol supplies them. The
renderer never emits one `<details>` block per delta.

`<details>` is a visual-folding mechanism only. It does not alter the storage,
webhook, API, search, audit-export, or email-notification size of a GitHub
comment. Therefore the first implementation must make the payload/result bounds
run-scoped and testable. Initial numerical limits remain calibration inputs,
but an unbounded result is never permitted even when the element is collapsed.

The earlier security posture remains explicit: Wrapper secrets are isolated
from the provider environment and structured credential/environment fields are
excluded, but a general free-form secret redactor is still deferred. Free-form
tool payload/result text can therefore repeat sensitive data despite model
best-effort. Until that filter exists, external dogfood must use non-sensitive
fixtures and this projection must not be described as a confidentiality
boundary. Byte truncation limits impact; it does not constitute redaction.

## Marker-free Publication Consequences

- `turn_id`, item IDs, revision, digest, and operation key remain local
  transport state only. They never appear in the GitHub body, visibly or
  invisibly.
- Once a remote comment ID is acknowledged, updates use that exact ID and the
  last acknowledged body digest. A foreign edit is a conflict.
- If create may have succeeded but its response was lost, Wrapper must not
  blindly create another comment. It may recover only from a unique canonical
  comment matching Wrapper author, target, intended body digest, and a bounded
  operation-time window. Zero, multiple, or unavailable matches remain
  `uncertain` and fail closed for operator reconciliation.
- FYI comments likewise contain only friendly prose and the canonical comment
  link. Dedupe uses local outbox state plus unique canonical author/target/body
  evidence, never a hidden ownership marker.
- Internal SQLite may retain the opaque ownership and outbox fields required
  for transport convergence. Removing them from GitHub is a publication-boundary
  change, not a decision to weaken local idempotency.
