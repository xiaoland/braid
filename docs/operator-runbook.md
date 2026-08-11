# Operator Runbook

This runbook prepares a disposable, real GitHub Issue-to-Draft-PR campaign. It
does not authorize or perform any external mutation by itself.

## Required isolation

1. Use a dedicated test GitHub App installed only on the approved repository.
   Grant Issues read/write, Pull requests read, and Metadata read. Subscribe to
   `issues`, `issue_comment`, `pull_request`, `pull_request_review`,
   `pull_request_review_comment`, and `pull_request_review_thread`.
2. Use separate GitHub identities for Wrapper comments and Agent `gh` actions.
   Give the Agent credential only the repository permissions required by the
   approved coding workflow; never pass the Wrapper private key or webhook
   secret into app-server.
3. Provision the Agent's branch and worktree before binding. Record
   `git worktree list --porcelain`, branch, HEAD, and `git status --short` for
   the primary, bootstrap, and candidate worktrees. The configured
   `provider_cwd` must be the candidate worktree.
4. Confirm the bounded section documented in
   `project-scope-collaboration.md` is present in this project's `AGENTS.md`,
   then point `collaboration_instructions` at that file. The runtime pins its
   digest and refuses silent instruction drift. Do not modify user-scope
   instructions for this campaign.
5. Create a private runtime directory outside every repository worktree. Copy
   `config.example.json` to `config.local.json`; store secret material only in
   its referenced environment variable or file.

## Preflight

```shell
pdm install
pdm lock --check
pdm run braid --help
pdm run braid config-check --config /absolute/path/config.local.json
pdm run braid probe-app-server \
  --codex /absolute/path/to/codex \
  --workspace /absolute/path/to/read-only-probe-workspace
```

The probe result must match all three configured app-server pin values. Check
that the configured state-database parent exists and the database does not
belong to another running Wrapper.

Before implementation, place a Human worktree-safety comment on the bound Issue:
identify the dedicated candidate branch, require it to be rooted at the intended
remote base, forbid mutation of the primary or Wrapper worktree, and require a
base-relative diff check before push or PR creation. This is a message and
constraint, not a Wrapper command; do not expose local absolute paths.

## Start and observe

Start `serve` with the dedicated Issue and local Wrangler executable. Do not
copy private key, token, webhook secret, provider transcript, or local absolute
worktree paths into Issue comments or evidence.

The first stdout JSON record reports the binding, opaque thread address, and
temporary webhook URL. Loopback health reports provider, reconciliation,
scheduler, active-turn, tunnel, and last bounded mirror state. A tunnel failure
changes health to unavailable; it is not evidence that GitHub was quiet.

The runtime restores the prior App webhook URL only on graceful shutdown. If
the process is killed, inspect the App configuration before restarting because
a Quick Tunnel URL is temporary. GitHub does not automatically redeliver a
failed webhook; periodic canonical reconciliation is the normal convergence
path.

This automatic path requires the GitHub App-level webhook configuration API.
The first external smoke's existing App returned HTTP 404 there, so the runtime
failed closed before publication. An operator-managed repository webhook plus a
separately supervised HTTP/2 Quick Tunnel is a viable diagnostic fallback, but
it is not yet a public runtime command and was not part of the passing Issue 23
oracle. Do not claim webhook acceptance from the manually signed tunnel ping.

## Exclusive handoff and rollback

- Stop bootstrap `B` and confirm its process and owner lease are gone before
  starting candidate `C` on the same copied binding state. Never run both.
- Preserve the stopped `B` source, config digest, protocol pin, and runtime
  directory. Migrate by copying stopped state into a distinct `C` directory;
  never change `B` in place.
- Rollback means stopping `C`, verifying exclusivity, and restarting untouched
  `B`. Neither process may modify the primary worktree.

## Acceptance boundary

Local preflight checks and health only diagnose implementation. Acceptance is the
real GitHub → Quick Tunnel → Wrapper → Codex app-server → GitHub journey defined
in [`acceptance.md`](acceptance.md), including the genuine Draft PR, Human
discussion and review, lifecycle edits, missed-webhook reconciliation,
restart/unknown behavior, raw/rendered mirror inspection, and protected
worktree snapshots.
