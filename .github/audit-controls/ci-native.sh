#!/bin/bash
# Run in an isolated Actions checkout at the candidate commit.
set -euo pipefail
repo="$PWD"
snapshot=$(mktemp -d)
trap 'rm -rf "$snapshot"' EXIT
git archive HEAD | tar -x -C "$snapshot"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$repo/target}"
cd "$snapshot"
python3 - <<'PY'
from pathlib import Path
for filename, original, wrong in [
    ("src/provider/pi.rs", '"agent_settled" => {', '"agent_settled" | "agent_end" => {'),
    ("src/provider/codex.rs", '"turn/started" => {', '"turn/started" | "error" => {'),
    ("src/telemetry.rs", 'github.comment.summary = evidence.github_summary,', 'github.comment.summary = evidence.github_body,'),
    ("src/tunnel.rs", '.header("X-GitHub-Event", "ping")', '.header("X-GitHub-Event", "unknown")'),
]:
    path = Path(filename)
    text = path.read_text()
    assert text.count(original) == 1, filename
    path.write_text(text.replace(original, wrong))
PY
filters=(provider::pi::tests::pi_settles_after_retry provider::codex::tests::retryable_errors telemetry::tests::telemetry_routes_complete_evidence_only_to_otlp tunnel::tests::signed_probe_requires)
if cargo test --locked --bin braid -- "${filters[@]}" >"$snapshot/red.log" 2>&1; then
  cat "$snapshot/red.log"
  exit 1
fi
cat "$snapshot/red.log"
for test_case in "${filters[@]}"; do grep -E "^test ${test_case}.*FAILED" "$snapshot/red.log"; done
for reason in 'intermediate frames must leave the turn open' 'willRetry' 'missing, incomplete, or misrouted github.comment.summary' 'signed ping must be durably admitted'; do
  grep -F "$reason" "$snapshot/red.log"
done
for file in src/provider/pi.rs src/provider/codex.rs src/telemetry.rs src/tunnel.rs; do
  cp "$repo/$file" "$file"
  cmp "$repo/$file" "$file"
done
cargo test --locked --bin braid -- "${filters[@]}"
if [[ -n "${BRAID_TEST_PI_INSTALL:-}" ]]; then
  export PI_OFFLINE=1
  for file in src/provider/pi.rs src/provider/factory.rs; do git -C "$repo" show "fe82de3:$file" > "$file"; done
  if cargo test --locked --bin braid failed_home_binding_stops_the_installed_execution_before_returning -- --ignored >"$snapshot/construction-red.log" 2>&1; then
    cat "$snapshot/construction-red.log"
    exit 1
  fi
  cat "$snapshot/construction-red.log"
  grep -F 'failed factory construction omitted the owned-stop receipt' "$snapshot/construction-red.log"
  for file in src/provider/pi.rs src/provider/factory.rs; do cp "$repo/$file" "$file"; cmp "$repo/$file" "$file"; done
  cargo test --locked --bin braid failed_home_binding_stops_the_installed_execution_before_returning -- --ignored
fi
