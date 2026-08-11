# Acceptance Helpers

This directory is the only home for executable Braid acceptance helpers while
the product workflow is still being discovered.

A helper belongs here only when it drives Braid through public boundaries:
real GitHub objects, the public `braid` CLI and health surface, a real webhook
ingress, and a real Codex app-server. Importing internal Braid modules, replacing
GitHub or app-server with fakes, or inspecting the local database does not make
a script black-box acceptance.

These files are operator tools. Running one is not sufficient evidence of
product acceptance. Each campaign must retain the external GitHub evidence,
timing observations, protected-worktree snapshots, and Human verdicts required
by [`docs/acceptance.md`](../../docs/acceptance.md).

No prior test qualified for migration into this directory. The file formerly
named `test_turn_mirror_black_box.py` directly instantiated internal storage,
controller, publisher, fake-provider, and fake-GitHub objects; it was a
component integration test and was retired with the rest of the suite.
