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
  credential-pattern scan. `xiaoland/braid` does not yet exist. The prior
  black-box surfaces still present are SVC Issues 22, 23, and 24 with four
  Wrapper comments. SVC has no repository webhook. GitHub developer settings
  show three acceptance-only Apps: `svc-issue20-agent`,
  `svc-issue20-wrapper`, and `svc-issue20-wrapper-v2`. The approved logo is a
  three-strand indigo/cyan/coral knot on a near-black square, representing the
  Human, GitHub, and Coding Agent being braided into one collaboration loop.
- **Next Step**: Commit the verified independent snapshot, create and push
  `xiaoland/braid`, then configure the new Braid App before removing the source
  subtree or deleting acceptance evidence.
