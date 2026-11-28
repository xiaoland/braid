#!/usr/bin/env bash
set -euo pipefail
file=src/store/mod.rs
restore() { git restore -- "$file"; }
trap restore EXIT
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
mutate() {
  python3 - "$file" "$1" "$2" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1])
old,new=sys.argv[2:]
s=p.read_text()
assert s.count(old)==1, (p,old,s.count(old))
p.write_text(s.replace(old,new))
PY
}
mutate ' || missing_terminals > 0' ''
catch_control Cargo.toml "$case_name" 'an unobserved turn terminal must publish the configured operational status'
restore
mutate 'AND NOT (ps.execution_stopped_at IS NOT NULL' 'AND NOT (0 AND ps.execution_stopped_at IS NOT NULL'
catch_control Cargo.toml restarted_worker_retires_a_stopped_unassignment_without_a_provider_terminal 'a stopped unassignment must retire instead of becoming a blocked lost turn'
catch_control Cargo.toml worker_preserves_a_failed_turn_during_unassignment_while_the_platform_fence_is_offline 'a stopped unassignment must never resume its old native session'
restore
cargo test --locked --bin braid -- worker_stops_a_running_unassigned_issue_only_after_its_quiet_window builtin_turn_runs_without_writer_service_and_cannot_mint_after_reassignment worker_preserves_a_failed_turn_during_unassignment_while_the_platform_fence_is_offline restarted_worker_retires_a_stopped_unassignment_without_a_provider_terminal retired_reset_history_cannot_replace_an_execution_or_block_a_finite_run managed_ingests_while_old_execution_fence_is_unavailable
