#!/usr/bin/env bash
set -euo pipefail
base=5455350a2fe1d95e45b1d973e2c086208e1827e5
source_dir=$PWD
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$source_dir/target}"
snapshot="$(mktemp -d)"
file=
restore() { [[ -z "$file" ]] || git restore -- "$file"; }
trap 'restore; rm -rf "$snapshot"' EXIT
if ! git cat-file -e "$base^{commit}" 2>/dev/null; then git fetch --no-tags origin "$base"; fi
git archive "$base" | tar -x -C "$snapshot"
python3 - "$snapshot" <<'PY'
from pathlib import Path
import sys
p=Path('src/group/continuous_tests.rs')
new=p.read_text()
old=Path(sys.argv[1])/p
source=old.read_text()
prefix=new[:new.index('#[tokio::test]')]
# The old neutral interface requires one additional unused method.
anchor='    async fn send_user_msg('
assert prefix.count(anchor)==1
prefix=prefix.replace(anchor, '    async fn interrupt(&self) -> Result<(), SessionError> { Ok(()) }\n'+anchor)
start=new.index('#[tokio::test]\nasync fn worker_stops_a_running_unassigned_issue_only_after_its_quiet_window')
end=new.find('#[tokio::test]',start+1)
case=new[start:end if end!=-1 else len(new)]
old.write_text(prefix+source[source.index('#[tokio::test]'):]+'\n'+case)
PY
catch_control() {
  local manifest="$1" case_name="$2" expected="$3" out
  out="$(mktemp)"
  if cargo test --locked --manifest-path "$manifest" --bin braid "$case_name" -- --nocapture >"$out" 2>&1; then cat "$out"; return 1; fi
  cat "$out"
  grep -E 'test .*FAILED' "$out" >/dev/null
  grep -F "$expected" "$out" >/dev/null
  rm -f "$out"
}
case_name=worker_stops_a_running_unassigned_issue_only_after_its_quiet_window
catch_control "$snapshot/Cargo.toml" "$case_name" 'settled unassignment must stop a running native turn'
file=src/store/mod.rs
python3 - <<'PY'
from pathlib import Path
p=Path('src/store/mod.rs')
s=p.read_text()
old="UPDATE turns SET lifecycle='unknown',ended_at=?2,"
assert s.count(old)==1
p.write_text(s.replace(old,"UPDATE turns SET lifecycle='interrupted',ended_at=?2,"))
PY
catch_control Cargo.toml "$case_name" 'closing the session alone cannot identify a turn outcome'
restore
file=src/group/worker.rs
python3 - <<'PY'
from pathlib import Path
p=Path('src/group/worker.rs')
s=p.read_text()
old='let terminal = active.as_deref_mut().and_then(RunningAgentTurn::terminal_after_close);'
assert s.count(old)==1
p.write_text(s.replace(old,'let terminal = active.as_deref_mut().and_then(RunningAgentTurn::terminal_after_close).filter(|_| false);'))
PY
for test_case in builtin_turn_runs_without_writer_service_and_cannot_mint_after_reassignment worker_preserves_a_failed_turn_during_unassignment_while_the_platform_fence_is_offline; do
  catch_control Cargo.toml "$test_case" 'observed provider terminal must be durable while the platform fence is offline'
done
restore
file=
