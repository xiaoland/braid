# Product Truth

Braid keeps a GitHub Issue and one local Coding Agent thread in one durable,
Human-visible collaboration loop. GitHub remains the team's place to discuss
and review work, while the Agent remains the decision-maker for implementation.

## Purpose and Pressure

The product addresses collaboration that is split between GitHub discussion and
an Agent's local working context. Humans need a durable place to settle intent,
observe progress, and review the resulting change without turning transport
automation into a second product authority.

## Claims and Evaluation

| Product claim | Rationale | Observable success | Expected evidence |
| --- | --- | --- | --- |
| The bound Issue is the design and acceptance source of truth. | Discussion must remain recoverable and separate from a candidate diff. | Material design changes are visible on the Issue and review changes remain attributable to the PR. | GitHub Issue/PR history and Human verdicts. |
| One Issue collaborates with one provider-owned Agent thread. | A single context avoids competing semantic state. | New and later activity reaches the same thread without an invented replacement. | Public binding/runtime evidence and provider lifecycle. |
| GitHub comments remain messages, not commands. | Humans must be able to discuss without accidentally granting execution authority. | Ordinary comments settle; only the exact trusted `@agent` hint changes scheduling latency. | Canonical GitHub events, actor permissions, and turn observations. |
| A turn is readable as one canonical GitHub comment. | Humans need visible progress and final evidence without hidden transport payloads. | Assistant text, provider-labelled summaries, and bounded tool evidence converge on one comment. | Raw/rendered comment bodies, edit history, and projection checks. |
| The Agent decides when and how to implement. | Readiness, worktree, branch, and PR choices are semantic coding decisions. | A sufficiently settled Issue leads to an associated Draft PR from the dedicated worktree; premature action is rejected. | Protected-worktree snapshots, native PR association, checks, and Human review. |

The complete product oracle is the real [end-to-end acceptance
contract](acceptance.md). Local checks and protocol probes diagnose the
prototype but do not establish product acceptance.

## Capabilities and Workflows

- **Discuss**: Humans settle a change on the Issue; associated-PR comments and
  reviews remain part of the same collaboration loop.
- **Implement**: Once the specification is sufficiently settled, the Agent
  works in its dedicated worktree, creates a natively associated Draft PR, and
  records material design changes on the Issue.
- **Review**: PR review steers the existing Agent thread while the PR carries
  candidate code, verification, and review response.
- **Observe**: Each turn has one canonical mirror. Mixed Issue/PR activity gets
  one short link-only FYI on each other participating surface; Braid never
  copies the response or interprets its prose.

## Rules and Scope

- GitHub owns Issue, PR, comment, review, association, identity, and permission
  state. The Agent owns semantic decisions and its authorized GitHub actions.
- The Coding Agent provider owns thread history, compaction, execution, and
  resume. Braid owns only transport, scheduling, and the remote turn
  projection.
- Braid does not interpret Human intent, judge readiness or acceptance, create
  PRs, choose branches, manage worktrees, merge, close Issues, or create
  replacement provider threads.
- The mirror never publishes raw chain-of-thought, arbitrary protocol
  envelopes, hidden debug payloads, provider IDs, or an ownership marker. Its
  bounded tool evidence is for Human observation, not confidentiality.
- Multiple candidate PRs, multiple Agent threads per Issue, raw repository-push
  routing, merge automation, and hosted production ingress are outside the
  current prototype.

## Business Language

- **Issue**: the GitHub discussion and product/design/acceptance authority.
- **Agent thread**: the provider-owned working context bound to one Issue.
- **Draft PR**: the candidate implementation surface natively associated with
  the Issue.
- **Turn mirror**: the Human-readable projection of one Agent turn onto one
  canonical GitHub comment.

