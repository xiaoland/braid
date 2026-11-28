#!/usr/bin/env bash
set -euo pipefail

root=$(git rev-parse --show-toplevel)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
cd "$root"
target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in
    /*) export CARGO_TARGET_DIR="$target_dir" ;;
    *) export CARGO_TARGET_DIR="$root/$target_dir" ;;
esac

# Both changes affect only this snapshot. They touch different operations, so
# each target can run against the same compiled test binary.
git archive HEAD | tar -x -C "$scratch"
cd "$scratch"
python3 - <<'PY'
from pathlib import Path

def replace(path, old, new):
    source = Path(path).read_text()
    if source.count(old) != 1:
        raise RuntimeError(f"expected one control site in {path}")
    Path(path).write_text(source.replace(old, new))

# Restore the pre-fix sample retention from bbc82f6^.
replace("src/github/rate_limit.rs",
        "self.resource = reset.and(resource);\n        self.used = used;",
        "self.resource = resource;\n        self.used = used.or(self.used);")
replace("src/writer/ensure.rs",
        "    let request_marker = format!",
        '    if receipt.stage == "pull_request_ready" {\n'
        '        return Err(GitHubError::GraphQl("mutation: lose persisted PR recovery".into()));\n'
        '    }\n    let request_marker = format!')
PY

budget=github::rate_limit::window_tests::used_delta_requires_two_comparable_complete_header_samples
restart=writer::tests::initial_ensure_bootstrap_pr_association_and_restart_do_not_duplicate_mutations
run_case() {
    cargo test --locked --bin braid "$1" -- --exact --color never
}

red() {
    local name=$1
    shift
    local log="$scratch/$name.red"
    if run_case "$name" >"$log" 2>&1; then
        cat "$log"
        echo "mutation survived: $name" >&2
        return 1
    fi
    cat "$log"
    grep -Fq "test $name ... FAILED" "$log"
    for diagnostic do
        grep -Fq -- "$diagnostic" "$log"
    done
}
red "$budget" 'used=Some(25), reset=Some(600), resource=Some("core")' 'left: Some(3)' 'right: None'
red "$restart" 'mutation: lose persisted PR recovery' 'left: "rejected"' 'right: "applied"'

for path in src/github/rate_limit.rs src/writer/ensure.rs; do
    git -C "$root" show "HEAD:$path" > "$path"
done
for name in "$budget" "$restart"; do
    run_case "$name" | tee "$scratch/$name.green"
    grep -Fq "test $name ... ok" "$scratch/$name.green"
done
