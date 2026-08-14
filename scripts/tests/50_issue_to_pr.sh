#!/bin/sh
set -eu

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

config=${BRAID_CONFIG:?BRAID_CONFIG must point to the real acceptance config}
braid=${BRAID_BIN:-braid}
repository=${BRAID_TEST_REPOSITORY:-xiaoland/braid}
issue_profile=${BRAID_TEST_ISSUE_PROFILE:-issue-codex}
keep=${BRAID_TEST_KEEP_FIXTURES:-0}
stamp=$(date -u +%Y%m%dT%H%M%SZ)
issue_number=
pull_number=
head_ref=
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-slice5.XXXXXX")

cleanup() {
    if [ "$keep" = "1" ]; then
        echo "keeping Slice 5 fixtures: issue=${issue_number:-none} pr=${pull_number:-none}" >&2
        return
    fi
    if [ -n "$pull_number" ]; then
        gh pr close "$pull_number" --repo "$repository" >/dev/null 2>&1 || true
    fi
    if [ -n "$head_ref" ]; then
        gh api --method DELETE "repos/$repository/git/refs/heads/$head_ref" >/dev/null 2>&1 || true
    fi
    if [ -n "$issue_number" ]; then
        gh issue close "$issue_number" --repo "$repository" --reason "not planned" >/dev/null 2>&1 || true
    fi
    rm -rf "$temporary_root"
}
trap cleanup EXIT HUP INT TERM

command -v "$braid" >/dev/null 2>&1 || [ -x "$braid" ] || fail "Braid binary is unavailable"
command -v gh >/dev/null 2>&1 || fail "gh is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"

"$braid" migrate apply --config "$config" >/dev/null
identity=$("$braid" github probe --config "$config" --repository "$repository" --json)
jq -e '.permissions.issues == "write" and .permissions.pull_requests == "write" and .permissions.contents == "write"' \
    >/dev/null <<EOF || fail "Braid App requires Issues, Pull requests, and Contents write"
$identity
EOF

issue_url=$(gh issue create \
    --repo "$repository" \
    --title "Braid Slice 5 write receipt $stamp" \
    --body "Disposable real-object fixture for the first Slice 5 braid gh / pr ensure vertical.")
issue_number=${issue_url##*/}

comment_json=$("$braid" gh comment create "$repository#$issue_number" \
    --config "$config" \
    --profile "$issue_profile" \
    --request-id "slice5-$stamp-implementation-request" \
    --body "Implementation Request: create one Draft PR for the public Slice 5 write/ensure acceptance vertical." \
    --json)
comment_id=$(printf '%s' "$comment_json" | jq -r '.remote_database_id')
receipt_id=$(printf '%s' "$comment_json" | jq -r '.intent_id')
profile_display=$("$braid" profile inspect --config "$config" --profile "$issue_profile" --json | jq -r .display_name)
[ -n "$comment_id" ] && [ "$comment_id" != null ] || fail "comment receipt omitted remote comment ID"
body=$(gh api "repos/$repository/issues/comments/$comment_id" --jq .body)
first=$(printf '%s\n' "$body" | sed -n '1p')
second=$(printf '%s\n' "$body" | sed -n '2p')
[ "$first" = "> **Braid Agent · $profile_display**" ] || fail "Braid attribution display name drifted"
[ "$second" = '> Issue Agent' ] || fail "Braid attribution role drifted"
"$braid" gh receipt "$receipt_id" --config "$config" --json | jq -e \
    '.lifecycle == "applied" and .operation == "comment_create" and .remote_database_id != null' \
    >/dev/null || fail "comment receipt did not converge"

head_ref="braid/implementation-request-$comment_id"
"$braid" gh pr ensure --comment "$comment_id" --config "$config" --json \
    >"$temporary_root/ensure-a.json" 2>"$temporary_root/ensure-a.err" &
first_pid=$!
"$braid" gh pr ensure --comment "$comment_id" --config "$config" --json \
    >"$temporary_root/ensure-b.json" 2>"$temporary_root/ensure-b.err" &
second_pid=$!
wait "$first_pid" || {
    cat "$temporary_root/ensure-a.err" >&2
    fail "first concurrent pr ensure failed"
}
wait "$second_pid" || {
    cat "$temporary_root/ensure-b.err" >&2
    fail "second concurrent pr ensure failed"
}

first_pr=$(jq -r '.pull_request_number' "$temporary_root/ensure-a.json")
second_pr=$(jq -r '.pull_request_number' "$temporary_root/ensure-b.json")
[ "$first_pr" = "$second_pr" ] || fail "concurrent pr ensure calls returned different PRs"
pull_number=$first_pr
[ "$pull_number" != null ] || fail "pr ensure receipt omitted PR number"
first_receipt=$(jq -r '.write.intent_id' "$temporary_root/ensure-a.json")
second_receipt=$(jq -r '.write.intent_id' "$temporary_root/ensure-b.json")
[ "$first_receipt" = "$second_receipt" ] || fail "concurrent pr ensure calls returned different receipts"

pr=$(gh pr view "$pull_number" --repo "$repository" --json isDraft,state,headRefName,baseRefName,url)
printf '%s' "$pr" | jq -e \
    --arg head "$head_ref" '.isDraft == true and .state == "OPEN" and .headRefName == $head' \
    >/dev/null || fail "ensured PR is not the expected open Draft/head"
count=$(gh pr list --repo "$repository" --state open --head "$head_ref" --json number --jq length)
[ "$count" = "1" ] || fail "expected exactly one open PR for deterministic head; found $count"

associated=$(gh api graphql \
    -f query='query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){closingIssuesReferences(first:100){nodes{number repository{nameWithOwner}}}}}}' \
    -f owner="${repository%%/*}" \
    -f name="${repository#*/}" \
    -F number="$pull_number" \
    --jq ".data.repository.pullRequest.closingIssuesReferences.nodes | any(.number == $issue_number and .repository.nameWithOwner == \"$repository\")")
[ "$associated" = true ] || fail "PR lacks the native association to the Implementation Request Issue"

head_sha=$(gh api "repos/$repository/git/ref/heads/$head_ref" --jq .object.sha)
commit=$(gh api "repos/$repository/git/commits/$head_sha")
parent_sha=$(printf '%s' "$commit" | jq -r '.parents[0].sha')
tree_sha=$(printf '%s' "$commit" | jq -r '.tree.sha')
parent_tree=$(gh api "repos/$repository/git/commits/$parent_sha" --jq .tree.sha)
[ "$tree_sha" = "$parent_tree" ] || fail "bootstrap commit changed the repository tree"
printf '%s' "$commit" | jq -e --arg id "$comment_id" \
    '.message == ("chore(braid): initialize implementation request " + $id)' \
    >/dev/null || fail "bootstrap commit message omitted the Implementation Request"

echo "PASS: Slice 5 write receipt + concurrent pr ensure"
echo "candidate=$($braid --version)"
echo "repository=$repository issue=$issue_number pr=$pull_number comment=$comment_id receipt=$first_receipt"
echo "scope=comment attribution/receipt, concurrent idempotency, same-tree bootstrap, Draft PR, native association"
echo "UNPROVEN: PR Agent Group, worktree, implementation diff, review, lifecycle, full Slice 5"
