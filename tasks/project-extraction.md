# Braid Project Extraction

- **Objective**: Extract Braid from the SVC repository into the independent
  `xiaoland/braid` project without losing the implemented turn-projection,
  transport, tests, or design evidence; replace acceptance-only GitHub
  identities with one product GitHub App carrying the Braid logo.
- **Guardrails**: Treat the current `agent-handoff/` working tree as the source
  snapshot, including its uncommitted task packets and black-box guard. Copy
  and verify the independent repository before removing the source directory.
  Never copy `config.local.json`, credentials, runtime databases, build output,
  caches, or virtual environments. Do not modify user-scope `AGENTS.md`.
  Delete only SVC Issues 22, 23, and 24 and the acceptance-only GitHub Apps
  `svc-issue20-agent`, `svc-issue20-wrapper`, and
  `svc-issue20-wrapper-v2`; preserve all unrelated GitHub objects and local
  worktrees. The old Apps' private keys and IDs must not enter the new repo.
- **Verification**: The independent checkout contains exactly the intended
  product files plus approved brand assets, has its own Git history and
  `origin` pointing to `xiaoland/braid`, passes the complete child test suite,
  lock check, CLI smoke, and diff check, and contains no secret-bearing local
  configuration. GitHub shows the new repository and Braid App with the logo;
  SVC Issues 22-24 and all three temporary Apps are absent. Only after those
  facts hold may `agent-handoff/` be removed from this SVC worktree.
- **Current Truth**: The independent local repository now exists at the
  approved Braid path. Its `main` branch preserves the three filtered subtree
  commits and overlays the exact current product snapshot plus three approved
  logo sizes; ignored caches, build artifacts, runtime state, credentials, and
  `config.local.json` were not migrated. A clean PDM install passes all 101
  tests, `pdm lock --check`, CLI help, `git diff --check`, and a tracked-file
  credential-pattern scan. The public repository is now
  [xiaoland/braid](https://github.com/xiaoland/braid), with `main` at the
  verified extraction commit. SVC Issues 22, 23, and 24 and their four Wrapper
  comments now return Gone. The clean acceptance worktree and local branch are
  removed, while the Issue 20/22/23/24 runtime databases, old App keys, token,
  and webhook secret were moved to Trash and are recoverable until Trash is
  emptied. SVC has no repository webhook. The three acceptance-only Apps
  `svc-issue20-agent`, `svc-issue20-wrapper`, and
  `svc-issue20-wrapper-v2` are deleted. The replacement product App is
  `Braid by xiaoland` (App ID 4558000), scoped to this account with Issues
  read/write, Pull requests read-only, and Metadata read-only. Its webhook is
  deliberately inactive until a stable ingress is supplied. The approved logo
  is a three-strand indigo/cyan/coral knot on a near-black square, representing
  the Human, GitHub, and Coding Agent being braided into one collaboration
  loop. GitHub now serves that logo for App ID 4558000 at both 120px and 70px;
  the settings page reports the image saved. A final developer-settings read
  shows only `Braid by xiaoland`; all three acceptance-only Apps are absent.
- **Next Step**: Extraction is complete. Continue future product work only in
  `xiaoland/braid`; the next product acceptance remains the separate full
  Issue-to-Draft-PR black-box campaign described by the existing packets.

## Extraction Result

- The SVC `main` branch never contained Braid. The obsolete remote feature
  branch `feat/github-collaboration-bootstrap` is deleted and had no PR.
- The former `agent-handoff/` directory is absent from the SVC worktree. Its
  removal commit changes only that path; all SVC framework files remain
  untouched.
- Braid's new `main` retains the filtered three-commit project history plus the
  verified extraction commit, rather than inheriting the unrelated SVC branch
  stack that invalidated the earlier PR 21.
