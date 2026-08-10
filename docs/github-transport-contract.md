# GitHub Transport Contract

GitHub remains canonical for Issue, PR, comment, review, and review-thread
state. The local database stores delivery identities, stable object references,
digests, cursors, and projection state only.

## Webhook Ingress

- Verify `X-Hub-Signature-256` as HMAC-SHA256 over the exact raw request bytes
  before parsing. Use constant-time comparison and reject a missing, malformed,
  or mismatched signature.
- `X-GitHub-Delivery` is the durable delivery GUID. A redelivery retains the
  GUID; the JSON body has no universal top-level event ID. Persist the delivery
  GUID together with `X-GitHub-Event`, action, repository/object node IDs,
  actor, version/digest, and canonical URL.
- Acknowledge only after verification and durable enqueue, and always within
  GitHub's ten-second deadline. GitHub does not automatically retry failed
  deliveries; reconciliation and explicit redelivery are required.
- Subscribe to `issues`, `issue_comment`, `pull_request`,
  `pull_request_review`, `pull_request_review_comment`, and
  `pull_request_review_thread`. The last event is the real-time source for
  review-thread resolved/unresolved lifecycle.
- Event enums are open to future additions. Unknown event/action/object shapes
  are recorded as unsupported transport evidence and fail closed; they are not
  forwarded through a generic serializer.

GitHub's official references are the authority:
[payloads](https://docs.github.com/en/webhooks/webhook-events-and-payloads),
[signature validation](https://docs.github.com/en/webhooks/using-webhooks/validating-webhook-deliveries),
and [delivery best practices](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks).

## Identity, Permission, and Comments

- Stable routing uses repository, Issue, PR, comment, review, and review-thread
  GraphQL node IDs. Database IDs/numbers remain API/display aids.
- `sender`, comment `user`, and `author_association` preserve provenance but do
  not by themselves grant urgent scheduling. For exact visible `@agent`, query
  the actor's current repository permission and accept only explicit
  `triage|write|maintain|admin`; unknown/custom/unavailable remains ordinary.
- The dedicated GitHub App needs `Issues: write`, `Pull requests: read`, and
  `Metadata: read` for the bootstrap boundary. Wrapper prose is created/edited
  through Issue-comment endpoints, which also cover PR conversation comments.
  The Agent uses separate `gh` authority for branch/PR actions.
- Wrapper-authored mirror/FYI comment IDs and digests are origin evidence. Their
  webhook echoes acknowledge local outbox operations and never wake the Agent.

## Reconciliation and Lifecycle

Webhook edits/deletes retain object references and edited payloads include the
previous body. Minimization has no Issue-comment webhook action, and review
resolution is a separate thread event, so canonical GraphQL reconciliation is
mandatory. Query latest comment `updatedAt/lastEditedAt/isMinimized`, review
state, and `reviewThreads.isResolved`; a missing object after prior observation
becomes a tombstone rather than an invented empty message.

Acceptance accelerates reconciliation to 60 seconds. The normal operational
interval remains configurable and independent from the 30-second quiet window.

## Native Issue-to-PR Association

Do not parse titles, branch names, ordinary mentions, timeline cross-references,
or connected events as the routing authority. Query the native GraphQL
connections:

- PR to Issue: `PullRequest.closingIssuesReferences`;
- Issue to PR: `Issue.closedByPullRequestsReferences` with closed PRs included.

Closing-keyword associations exist only when the PR targets the repository's
default branch. One PR can link multiple Issues and one Issue can expose multiple
PRs; the bootstrap fails closed unless exactly one current associated PR matches
the bound Issue. Supporting multiple candidates is a later architecture change.

The Agent workflow must create this native association when it creates the
Draft PR. See GitHub's [linking contract](https://docs.github.com/en/issues/tracking-your-work-with-issues/using-issues/linking-a-pull-request-to-an-issue)
and the [GraphQL Issue](https://docs.github.com/en/graphql/reference/issues) and
[PullRequest](https://docs.github.com/en/graphql/reference/pulls) references.

REST calls pin an explicit supported `X-GitHub-Api-Version`; webhook and GraphQL
schemas are revalidated against current official documentation before a pinned
bootstrap is promoted.
