# GitHub Boundary Contract

GitHub is canonical for Work Item content and collaboration lifecycle. Braid
uses webhooks for low-latency observation and complete GraphQL/REST rereads for
truth. Arrival order is never canonical order.

## App Identity and Permissions

The configured Braid Agent App supplies:

- the stable Bot/App-authored write and reaction identity;
- repository installation scope;
- webhook deliveries;
- canonical read and `braid gh` write credentials.

An installation is an Issue assignment target only when GitHub has provisioned
it as an Agent App. Ordinary GitHub Apps are not standard user assignees.

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

## Issue and PR Activation

GitHub's Agent App assignment is a special product capability, not a permission
granted to every ordinary GitHub App. A live 2026-08-13 probe against the Braid
installation confirmed that adding its Bot login through the standard Issue
assignee API is rejected with HTTP 403. Braid therefore exposes two Issue
activation modes:

1. when the installation is a provisioned Agent App, an `issues.assigned`
   delivery starts a generation only after a canonical reread confirms the
   exact assignee; the activation creates an idle session and does not invent a
   turn;
2. otherwise, the first Trusted Braid Mention on a dormant Issue produces one
   `ActivationIntent` and preserves that same comment as an urgent Wake Event,
   so materialization is followed by the first turn.

Native unassignment is likewise available only in the first mode and must be
confirmed from canonical assignees before entering debounce. The fallback is
not presented as a fabricated assignment.

GitHub does not expose a general native PR Agent assignee workflow. PR
activation therefore has two sources that produce the same durable
`ActivationIntent`:

1. a Trusted Braid Mention in a PR conversation comment;
2. successful convergence of `braid gh pr ensure` for an Issue comment ID.

The configured handle defaults to `@braid`. Permission is checked at delivery
time with current repository permission and requires `MAINTAIN` or `ADMIN`;
`author_association`, sender login, and stale cached permission are insufficient.

GitHub's special Agent App entry points are documented in
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
retains a deleted body as Context. Older webhook payloads remain delivery
evidence. Ordering is applied only where GitHub exposes comparable versions;
otherwise a lifecycle event causes a canonical reread instead of a guessed
lexicographic comparison.

Cross-surface description detection is Issue-owned rather than inferred from
the generic root `updatedAt`: Braid compares the exact visible Issue description
after HTML-comment removal once, then fans a real change out to every active
direct PR association. Webhook `issues.edited` uses the exact `changes.body`
signal. The new visible description and any derived PR events are committed
together, so repeated delivery, a metadata-only edit, or another Context read
cannot manufacture a second invalidation.

Reconciliation records a previously unseen review thread without creating a
second Wake: the thread's first visible comment/review delivery already owns
that Wake. Only a known thread changing from resolved to unresolved creates the
thread-level Wake; resolved remains a Hard Invalidation.

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

The first shipped write vertical is intentionally small and recoverable:

```shell
braid gh comment create owner/repository#123 \
  --config /absolute/path/to/braid.toml \
  --profile issue-codex \
  --request-id stable-agent-message-key \
  --body 'Concise public update'

braid gh pr ensure --comment 123456789 \
  --config /absolute/path/to/braid.toml
```

`comment create` derives the public role from the canonical Issue/PR target and
the Profile tags. The writer prepends one attribution block and removes exact
repetitions of that generated block from the start of the supplied body, so a
caller retry or model mistake cannot duplicate it. `--request-id` is optional;
exact attributed body content provides the fallback retry identity, while an
explicit value lets the Agent name retry intent. Command results expose only
bounded operation/target/Profile/lifecycle and semantic GitHub comment/PR
references. Internal write IDs, node IDs, digests, and retry counters remain in
SQLite and never enter Agent tool output. `pr ensure` accepts an optional
explicit `--head`, then the
sole same-repository Development branch, then its deterministic
request-comment-derived branch.

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
an uncertain comment create accepts only one matching App author, target, exact
body, and creation-window candidate; multiple candidates become `ambiguous`.
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
