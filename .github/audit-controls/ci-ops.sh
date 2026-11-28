#!/bin/sh
# From the checkout root, in an isolated CI job.
set -eu
python3 - <<'PY'
from pathlib import Path
import re
import subprocess

def control(file, before, after, suite, case, expected, pos):
    path = Path(file)
    original = path.read_bytes()
    assert original.count(before.encode()) == 1, f"{file}: mutation anchor changed"
    try:
        path.write_bytes(original.replace(before.encode(), after.encode(), 1))
        cmd = ["cargo", "test", "--locked", "--test", suite, case, "--", "--exact"]
        result = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        print(result.stdout, flush=True)
        # The source line and the exact values distinguish the target assertion from setup errors.
        line = next(i for i, s in enumerate(Path(f"tests/{suite}.rs").read_text().splitlines(), 1)
                    if pos in s)
        assert result.returncode != 0 and f"test {case} ... FAILED" in result.stdout
        assert f"tests/{suite}.rs:{line}:" in result.stdout and re.search(expected, result.stdout)
    finally:
        path.write_bytes(original)

control("src/cli/status.rs", "config_schema: config.schema_version,", "config_schema: 3,",
        "multi_repo", "instance_selectors_route_to_the_expected_config_and_doctor_names_cross_instance_conflicts",
        r"left: Number\(3\)\s+right: 2", 'assert_eq!(status["config_schema"], 2)')

branch = "    if let Some(event_id) = pending {\n        transaction.commit()?;\n        return Ok(event_id);"
duplicate = """    if let Some(event_id) = pending {
        let duplicate = Uuid::now_v7().to_string();
        transaction.execute("INSERT INTO wake_batches(batch_id,work_item_node_id,event_count,quiet_deadline,lifecycle,created_at,updated_at)
             VALUES(?1,?2,1,?3,'consumed',?3,?3)", params![duplicate,work_item_node_id,now])?;
        transaction.execute("INSERT INTO wake_batch_events(batch_id,event_id,ordinal) VALUES(?1,?2,0)", params![duplicate,event_id])?;
        transaction.commit()?;
        return Ok(event_id);"""
control("src/store/mod.rs", branch, duplicate, "assignment",
        "assignment_reactivation_queues_one_wake_and_preserves_the_blocked_generation",
        r"left: 2\s+right: 1", "assert_eq!(batches, 1)")
PY
