#!/usr/bin/env bash
set -euo pipefail
file=
restore() { [[ -z "$file" ]] || git restore -- "$file"; }
trap restore EXIT
mutate() {
  python3 - "$file" "$1" "$2" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1])
s=p.read_text()
old,new=sys.argv[2:]
assert s.count(old)==1, (p,old,s.count(old))
p.write_text(s.replace(old,new))
PY
}
catch_control() {
  local test_case="$1" reason="$2" out
  out="$(mktemp)"
  if cargo test --locked --bin braid "$test_case" -- --nocapture >"$out" 2>&1; then
    cat "$out"
    return 1
  fi
  cat "$out"
  grep -E 'test .*FAILED' "$out" >/dev/null
  grep -F "$reason" "$out" >/dev/null
  rm -f "$out"
  restore
  file=
}
file=src/group/worker.rs
mutate 'self.quiesce_session(id).await? || recorded_stop' 'self.quiesce_session(id).await? || (recorded_stop && false)'
catch_control persisted_idle_reset_uses_only_confirmed_native_stop_after_restart 'right: "applied"'
file=src/store/reconciliation.rs
mutate 'CASE WHEN ?3 THEN ?2 ELSE reconciled_at END' 'CASE WHEN ?3 THEN ?2 ELSE NULL END'
catch_control failed_traversal_backs_off_without_losing_the_last_success 'due_ids(&db, 1899)'
file=src/store/mod.rs
mutate '    if before.pending.is_empty() {' '    if before.pending.is_empty() {
        create_backup(&open_read_only(database)?, backups, before.current_schema)?;'
catch_control v1_upgrade_preserves_the_association_and_comment_and_takes_only_one_backup 'braid.before-v'
for old_table in associations canonical_objects; do
  file=src/store/mod.rs
  mutate '    let after = plan(database)?;' "    if before.current_schema == 1 { connection.execute_batch(\"DELETE FROM ${old_table}\")?; }
    let after = plan(database)?;"
  catch_control v1_upgrade_preserves_the_association_and_comment_and_takes_only_one_backup 'QueryReturnedNoRows'
done
