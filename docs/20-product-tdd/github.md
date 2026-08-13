# GitHub Boundary Contract

GitHub is canonical for Work Item content and collaboration lifecycle. Braid
uses webhooks for low-latency observation and complete GraphQL/REST rereads for
truth. Arrival order is never canonical order.

## App Identity and Permissions

The configured Braid Agent App supplies:

- the Issue assignment target and stable Bot/App-authored comment identity;
- repository installation scope;
- webhook deliveries;
- canonical read and `braid gh` write credentials.

MVP App permissions are:

- Issues: write;
- Pull requests: write;
- Contents: write, used by `pr ensure` only for remote refs and an empty
  bootstrap commit when the head has no diff yet;
- Metadata: read;
- Projects: read when Project V2 fields are enabled.

The Agent may separately use its own `gh` and Git credentials. Braid does not
restrict them. A Profile may name one stable GitHub actor node identity; events
whose current action actor exactly matches it are Agent-origin for that Profile.
Writes executed through Braid's durable outbox are correlated Agent-origin as
well. A direct write under any other or shared Human identity is processed as
external activity.

## Webhook Ingress

- Compute HMAC-SHA256 over the exact request bytes and verify
  `X-Hub-Signature-256` in constant time before JSON parsing.
- Persist `X-GitHub-Delivery`, `X-GitHub-Event`, action, repository/object IDs,
  actor, receipt time, and raw payload before acknowledging. Redelivery retains
  the delivery GUID.
- Return 2xx within GitHub's ten-second deadline only after durable ingest.
  GitHub does not automatically retry failed deliveries; reconciliation and
  explicit redelivery remain required.
- Unknown event/action/union variants are durably recorded and trigger
  reconciliation. They are never serialized generically into Agent input.

Subscribe to:

- `issues` and `issue_comment`;
- `issue_dependencies`;
- `pull_request`, `pull_request_review`,
  `pull_request_review_comment`, and `pull_request_review_thread`;
- `project_v2_item` when Projects are enabled;
- repository/installation lifecycle needed to stop routing on uninstall or
  permission loss.

The official [webhook payload](https://docs.github.com/en/webhooks/webhook-events-and-payloads),
[signature](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries),
and [delivery](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks)
contracts are the source authority.

## Assignment and PR Activation

An `issues.assigned` delivery starts a new Issue generation only when its
assignee matches the configured Braid Agent App identity and the canonical
Issue reread confirms the assignment. Unassignment is likewise confirmed from
canonical assignees and enters debounce.

GitHub documents assigning Agent Apps to Issues, but not a general native PR
assignee workflow. PR activation therefore has two sources that produce the
same durable `ActivationIntent`:

1. a Trusted Braid Mention in a PR conversation comment;
2. successful convergence of `braid gh pr ensure` for an Issue comment ID.

The configured handle defaults to `@braid`. Permission is checked at delivery
time with current repository permission and requires `MAINTAIN` or `ADMIN`;
`author_association`, sender login, and stale cached permission are insufficient.

GitHub's current Agent App entry points are documented in
[Using agent apps](https://docs.github.com/en/copilot/how-tos/use-copilot-agents/cloud-agent/use-agent-apps).

## Canonical Reads

GraphQL is the primary Context read because it exposes stable node IDs,
minimization, edit timestamps, Project V2, relationships, linked branches, and
review-thread resolution. Braid paginates every connection to completion:

- Issue root, assignees, labels, milestone, Issue Type;
- Project V2 items and every non-empty field-value union;
- parent, subIssues, blockedBy, blocking, duplicate/canonical timeline evidence;
- `linkedBranches` Development refs;
- Issue comments with author, full database ID, created/updated/last-edited,
  minimization/reason, pinned state, and Markdown body;
- PR root, labels/assignees/milestone/projects, base/head ref names;
- native associated Issues through `closingIssuesReferences` and the reciprocal
  Issue connection, with 1:N and N:1 allowed;
- PR conversation comments, reviews, and every review thread/comment page with
  resolution/collapse/outdated/path/line metadata.

REST supplements GraphQL for endpoints or lifecycle evidence not exposed in a
stable query. Requests pin an explicit supported API version.

On 2026-08-13 the full selection was executed read-only against a live public
`openai/codex` Issue and PR. The Issue query returned labels/comments and
successfully selected Issue Type, Projects, parent/sub/dependency/linked-branch
connections; the PR query returned conversation, review, and review-thread
connections. Every pageInfo flag was checked. This proves field compatibility,
not the content of future repositories.

## Native Association

PR Context includes every direct native association. Braid queries
`PullRequest.closingIssuesReferences` and
`Issue.closedByPullRequestsReferences(includeClosedPrs: true)`; it does not
infer association from title, branch name, ordinary `#123` text, timeline
cross-reference, or commit message.

`pr ensure` establishes a GitHub-native link, preferably through the explicit
Development relationship supported by the target repository and otherwise a
valid closing reference targeting the default branch. Multiplicity is legal.
Removing or changing an edge invalidates affected PR Contexts.

See GitHub's [linking contract](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue),
[Issue schema](https://docs.github.com/en/graphql/reference/issues), and
[Pull request schema](https://docs.github.com/en/graphql/reference/pulls).

## Reconciliation and Tombstones

Webhook is an observation, not a complete history. Reconciliation runs every
60 seconds by default, separately from debounce, and after reconnect, unknown
variants, delivery gaps, or outbox uncertainty. It rereads active Work Items,
direct associations, and all Context connections.

An object previously observed but now missing becomes a tombstone carrying only
stable ID, last known author/time metadata, and `deleted`. Local state never
retains a deleted body as Context. Older webhook payloads are stored as evidence
but cannot replace a newer canonical digest/version.

## `braid gh` and Outbox

`braid gh` follows familiar GitHub target/flag conventions and implements the
MVP write subset under the Braid App identity:

- comment create/edit/delete/minimize/unminimize;
- Issue edit/label/close/reopen;
- PR edit/label/ready/draft/close/reopen;
- review reply/resolve/unresolve;
- idempotent `pr ensure` keyed by the triggering Issue comment ID.

The current Work Item is only a default target. Missing verbs are implementation
backlog, not a permission policy, and ordinary `gh` remains available.

`pr ensure` first selects an explicit requested head, the sole unambiguous
Development linked branch, or a deterministic request-comment-derived ref. It
compares that head to the intended base. When GitHub has no differing commit,
Braid uses the Git Data API to create a commit with the current head tree and
parent, message `chore(braid): initialize implementation request <comment>`, and
stable Braid App authorship, then advances/creates the ref without force. A
concurrent Human advance causes reread rather than overwrite. The empty commit
is the minimum public fact needed because GitHub cannot open a PR without a
head/base difference; it changes no repository file. Braid then creates the
Draft PR, establishes native association, and records ActivationIntent. GitHub
requires Contents write for App installation
[commit](https://docs.github.com/en/rest/git/commits) and
[reference](https://docs.github.com/en/rest/git/refs) creation.

Every Braid-owned mutation first commits an immutable write intent. States are
`pending`, `sending`, `uncertain`, `applied`, `conflict`, `ambiguous`,
`rejected`, and `superseded`. A timeout becomes `uncertain`; Braid rereads
canonical state before retrying. Known-object updates/reactions converge by
desired state. GitHub creates have no persisted queryable exactly-once key, so
an uncertain comment create accepts only one matching App author/target/body
digest/time-window candidate; multiple candidates become `ambiguous`.
`pr ensure` additionally reconciles the request comment ID, deterministic head
ref, bootstrap commit ancestry, open PR head/base, and native associations.

## Reactions and Status Comments

After durable ingest Braid adds `eyes` to each new external comment. For the
exact Trusted Braid Mention only, it maintains the reaction lifecycle defined
in [`lifecycle.md`](lifecycle.md). GitHub reaction create is treated as
idempotent; removal lists and deletes only Braid-owned reaction IDs.

Operational Status Comments are App-authored projections keyed by
`(profile, surface, assignment generation)`. Braid edits one stable comment per
key, excludes its ID from Context, and never hides debug metadata in the body.
Profiles choose Issue, PR, or both direct surfaces. Status covers context
pressure/unavailability, provider/transport unknown, blocked Profile/worktree,
and ambiguous/rejected Braid writes.
