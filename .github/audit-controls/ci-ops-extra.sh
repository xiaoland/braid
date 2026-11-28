#!/bin/sh
# Run in an isolated checkout root. Exact panic line + output identify each guard.
set -eu
python3 - <<'PY'
from pathlib import Path
import subprocess

cases = [
    ("src/setup.rs", "    user_home.save_registry(&registry)?;", "",
     ["--test", "headless_setup"], "no_browser_setup_can_be_completed_from_callback_url",
     "tests/headless_setup.rs", 'let summary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();',
     -1, ['unknown instance "demo"', "no instances registered"]),
    ("src/worktree.rs", "    exclude_private_notes(request, &source)?;", "",
     ["--bin", "braid"], "worktree::tests::existing_target_must_share_the_requested_source_repository",
     "src/worktree/tests.rs", 'assert_eq!(String::from_utf8(status.stdout).unwrap()', 0,
     [r'left: "?? .braid/private-note\n?? change.txt\n"', r'right: "?? change.txt\n"']),
    ("src/health.rs", "if error.is_some() || current.last_error == prior_error {", "if error.is_some() {",
     ["--bin", "braid"], "health::tests::provider_recovery_clears_its_own_diagnostic_and_preserves_another_workers_error",
     "src/health/tests.rs", "assert_eq!(recovered.last_error.as_deref(), other_error);", 0,
     ['left: Some("provider failed")', 'right: None']),
]

def run(target, case):
    return subprocess.run(["cargo", "test", "--locked", *target, case, "--", "--exact"],
                          stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)

for file, original, mutation, target, case, test_source, anchor, offset, evidence in cases:
    path = Path(file)
    saved = path.read_bytes()
    assert saved.count(original.encode()) == 1, f"{file}: mutation anchor changed"
    lines = [i for i, line in enumerate(Path(test_source).read_text().splitlines(), 1) if anchor in line]
    assert len(lines) == 1, f"{test_source}: assertion anchor changed"
    try:
        path.write_bytes(saved.replace(original.encode(), mutation.encode(), 1))
        result = run(target, case)
        print(result.stdout, flush=True)
        assert result.returncode != 0 and f"test {case} ... FAILED" in result.stdout
        for detail in [f"{test_source}:{lines[0]+offset}:", *evidence]:
            assert detail in result.stdout, f"{case}: missing expected failure {detail!r}"
    finally:
        path.write_bytes(saved)

for _, _, _, target, case, *_ in cases:
    result = run(target, case)
    print(result.stdout, flush=True)
    assert result.returncode == 0 and f"test {case} ... ok" in result.stdout
PY
