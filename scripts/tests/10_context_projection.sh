#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-context-projection.XXXXXX")
trap 'rm -rf "$temporary_root"' EXIT HUP INT TERM

if [ "$#" -lt 3 ]; then
    echo "UNAVAILABLE: usage: $0 ABSOLUTE_CONFIG OWNER/REPO#ISSUE OWNER/REPO#PR [ARCHIVE]" >&2
    exit 2
fi

for required in \
    BRAID_EXPECT_VISIBLE_TEXT \
    BRAID_EXPECT_FILTERED_TEXT \
    BRAID_EXPECT_FOLDED_REFERENCE \
    BRAID_EXPECT_DELETED_REFERENCE \
    BRAID_EXPECT_PAGINATED_REFERENCE \
    BRAID_EXPECT_METADATA_LINE \
    BRAID_EXPECT_ISSUE_PR_COUNT \
    BRAID_EXPECT_CLOSED_ISSUE_REFERENCE \
    BRAID_EXPECT_CLOSED_ISSUE_FILTERED_TEXT \
    BRAID_EXPECT_PR_ISSUE_COUNT
do
    eval "value=\${$required:-}"
    if [ -z "$value" ]; then
        echo "UNAVAILABLE: $required must describe the controlled real fixture" >&2
        exit 2
    fi
done

config=$1
issue=$2
pull_request=$3
archive=${4:-}
if [ -z "$archive" ]; then
    archive=$(BRAID_DIST_DIR="$temporary_root/dist" "$repository_root/scripts/package.sh")
fi
case "$archive" in
    /*) ;;
    *) archive="$repository_root/$archive" ;;
esac

install_root="$temporary_root/install"
mkdir -p "$install_root"
tar -C "$install_root" -xzf "$archive"
package_root=$(find "$install_root" -mindepth 1 -maxdepth 1 -type d -name 'braid-*' | head -1)
braid="$package_root/bin/braid"

"$braid" migrate apply --config "$config"
repository=${issue%#*}
"$braid" github probe --config "$config" --repository "$repository" --json > "$temporary_root/github.json"

"$braid" context issue "$issue" --config "$config" --page-size 1 > "$temporary_root/issue-a.md"
"$braid" context issue "$issue" --config "$config" --page-size 1 > "$temporary_root/issue-b.md"
cmp "$temporary_root/issue-a.md" "$temporary_root/issue-b.md"

"$braid" context pr "$pull_request" --config "$config" --page-size 1 > "$temporary_root/pr-a.md"
"$braid" context pr "$pull_request" --config "$config" --page-size 1 > "$temporary_root/pr-b.md"
cmp "$temporary_root/pr-a.md" "$temporary_root/pr-b.md"

"$braid" context issue "$issue" --config "$config" --page-size 1 --json > "$temporary_root/issue.json"
jq -e '.bytes > 0 and (.pressure == "normal" or .pressure == "soft") and (has("revision") | not)' \
    "$temporary_root/issue.json" >/dev/null

grep -F -q "$BRAID_EXPECT_VISIBLE_TEXT" "$temporary_root/issue-a.md"
if grep -F -q "$BRAID_EXPECT_FILTERED_TEXT" "$temporary_root/issue-a.md"; then
    echo "HTML-comment content leaked into Issue Context" >&2
    exit 1
fi
grep -F -q "$BRAID_EXPECT_FOLDED_REFERENCE" "$temporary_root/issue-a.md"
grep -F -q "$BRAID_EXPECT_DELETED_REFERENCE" "$temporary_root/issue-a.md"
grep -F -q "$BRAID_EXPECT_PAGINATED_REFERENCE" "$temporary_root/issue-a.md"
grep -F -q "$BRAID_EXPECT_METADATA_LINE" "$temporary_root/issue-a.md"

issue_pr_count=$(grep -o 'GitHub PR:' "$temporary_root/issue-a.md" | wc -l | tr -d ' ')
test "$issue_pr_count" = "$BRAID_EXPECT_ISSUE_PR_COUNT"

issue_count=$(grep -c '^# GitHub Issue: ' "$temporary_root/pr-a.md")
test "$issue_count" = "$BRAID_EXPECT_PR_ISSUE_COUNT"
grep -F -q "$BRAID_EXPECT_CLOSED_ISSUE_REFERENCE" "$temporary_root/pr-a.md"
if grep -F -q "$BRAID_EXPECT_CLOSED_ISSUE_FILTERED_TEXT" "$temporary_root/pr-a.md"; then
    echo "closed Issue body leaked into PR Context" >&2
    exit 1
fi

tiny_config="$temporary_root/tiny.toml"
sed 's/^github_context_hard_bytes = .*/github_context_hard_bytes = 1/' "$config" > "$tiny_config"
if "$braid" context issue "$issue" --config "$tiny_config" --page-size 1 > "$temporary_root/tiny.out" 2>&1; then
    echo "hard Context budget unexpectedly emitted a partial Context" >&2
    exit 1
fi
grep -q 'above the Profile hard limit' "$temporary_root/tiny.out"

echo "real Context projection diagnostics passed"
