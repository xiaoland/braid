#!/bin/sh
set -eu

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

source_config=${BRAID_CONFIG:?BRAID_CONFIG must point to the real acceptance config}
braid=${BRAID_BIN:-braid}
repository=${BRAID_TEST_REPOSITORY:-xiaoland/braid}
issue_profile=${BRAID_TEST_ISSUE_PROFILE:-issue-codex}
keep=${BRAID_TEST_KEEP_FIXTURES:-0}
ingress_address=${BRAID_TEST_INGRESS:-127.0.0.1:18090}
health_address=${BRAID_TEST_HEALTH:-127.0.0.1:18091}
health_url="http://$health_address/healthz"
stamp=$(date -u +%Y%m%dT%H%M%SZ)
issue_number=
pull_number=
head_ref=
runtime_pid=
worktree_path=
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-slice5.XXXXXX")
runtime_root="$temporary_root/runtime"
source_checkout="$temporary_root/source"
config="$temporary_root/braid.toml"
runtime_log="$temporary_root/runtime.log"

stop_runtime() {
    [ -n "$runtime_pid" ] || return 0
    kill -0 "$runtime_pid" 2>/dev/null || return 0
    kill -INT "$runtime_pid" 2>/dev/null || true
    for _ in $(seq 1 15); do
        kill -0 "$runtime_pid" 2>/dev/null || {
            wait "$runtime_pid" 2>/dev/null || true
            return 0
        }
        sleep 1
    done
    kill -TERM "$runtime_pid" 2>/dev/null || true
    wait "$runtime_pid" 2>/dev/null || true
}

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    stop_runtime
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
    if [ "$status" -ne 0 ]; then
        echo "Slice 5 runtime log follows" >&2
        [ -f "$runtime_log" ] && tail -200 "$runtime_log" >&2 || true
    fi
    rm -rf "$temporary_root"
}
trap cleanup EXIT HUP INT TERM

command -v "$braid" >/dev/null 2>&1 || [ -x "$braid" ] || fail "Braid binary is unavailable"
command -v gh >/dev/null 2>&1 || fail "gh is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"
command -v curl >/dev/null 2>&1 || fail "curl is unavailable"
command -v git >/dev/null 2>&1 || fail "git is unavailable"
[ -n "${BRAID_WEBHOOK_SECRET:-}" ] || fail "BRAID_WEBHOOK_SECRET is required by the runtime"
[ "$source_config" = "${source_config#/}" ] && fail "BRAID_CONFIG must be absolute"
[ -f "$source_config" ] || fail "BRAID_CONFIG does not exist"

pr_profile=$("$braid" config check --config "$source_config" --json | jq -er .default_pr_profile)
mkdir -p "$runtime_root/state/backups"
runtime_root_real=$(cd "$runtime_root" && pwd -P)
gh repo clone "$repository" "$source_checkout" -- --quiet
awk \
    -v root="$runtime_root" \
    -v database="$runtime_root/state/braid.sqlite3" \
    -v backups="$runtime_root/state/backups" \
    -v ingress="$ingress_address" \
    -v health="$health_address" \
    -v pr_profile="$pr_profile" \
    -v source="$source_checkout" '
    /^\[runtime\]$/ { section="runtime"; print; next }
    /^\[server\]$/ { section="server"; print; next }
    /^\[\[profiles\]\]$/ { section="profile"; profile=""; print; next }
    /^\[/ && $0 != "[runtime]" && $0 != "[server]" && $0 != "[[profiles]]" { section="other" }
    section == "runtime" && /^root = / { printf "root = \"%s\"\n", root; next }
    section == "runtime" && /^database = / { printf "database = \"%s\"\n", database; next }
    section == "runtime" && /^backups = / { printf "backups = \"%s\"\n", backups; next }
    section == "runtime" && /^auto_migrate = / { print "auto_migrate = false"; next }
    section == "server" && /^ingress = / { printf "ingress = \"%s\"\n", ingress; next }
    section == "server" && /^health = / { printf "health = \"%s\"\n", health; next }
    section == "profile" && /^id = / {
        profile=$0
        gsub(/^id = \"|\"$/, "", profile)
        print
        next
    }
    section == "profile" && profile == pr_profile && /^workspace = / {
        printf "workspace = \"%s\"\n", source
        next
    }
    { print }
' "$source_config" > "$config"

"$braid" migrate apply --config "$config" >/dev/null
identity=$("$braid" github probe --config "$config" --repository "$repository" --json)
jq -e '.permissions.issues == "write" and .permissions.pull_requests == "write" and .permissions.contents == "write"' \
    >/dev/null <<EOF || fail "Braid App requires Issues, Pull requests, and Contents write"
$identity
EOF

binary_directory=$(dirname "$braid")
PATH="$binary_directory:$PATH" BRAID_CONFIG="$config" \
    BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" \
    "$braid" serve --config "$config" >"$runtime_log" 2>&1 &
runtime_pid=$!
for _ in $(seq 1 120); do
    if curl -fsS "$health_url" 2>/dev/null | \
        jq -e '.ready == true and .provider == "connected"' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during PR Agent startup"
    sleep 1
done
curl -fsS "$health_url" | jq -e '.ready == true and .provider == "connected"' >/dev/null || \
    fail "Braid did not become provider-ready"

fixture_path="acceptance/slice5-$stamp.md"
fixture_marker="Braid Slice 5 PR Agent $stamp"

issue_url=$(gh issue create \
    --repo "$repository" \
    --title "Braid Slice 5 write receipt $stamp" \
    --body "Disposable real-object fixture. Implement by creating \`$fixture_path\` containing exactly \`$fixture_marker\`, then commit, push the PR branch, verify the diff, and publish one concise PR comment.")
issue_number=${issue_url##*/}

comment_json=$("$braid" gh comment create "$repository#$issue_number" \
    --config "$config" \
    --profile "$issue_profile" \
    --request-id "slice5-$stamp-implementation-request" \
    --body "Implementation Request: implement the exact bounded file change in this Issue description, verify it, push it, and report concisely on the Draft PR." \
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

for _ in $(seq 1 240); do
    group=$("$braid" status --config "$config" --json | jq -c \
        --argjson number "$pull_number" --arg profile "$pr_profile" \
        '.transport.agent_groups[]? | select(.work_item_kind == "pr" and .work_item_number == $number and .profile_id == $profile)' | tail -1)
    if [ -n "$group" ] && printf '%s' "$group" | jq -e \
        '.assignment_lifecycle == "active" and .provider_session_id != null and .worktree_lifecycle == "active" and .worktree_path != null' >/dev/null; then
        worktree_path=$(printf '%s' "$group" | jq -r .worktree_path)
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited before PR Agent materialization"
    sleep 1
done
[ -n "$worktree_path" ] || fail "PR Agent/worktree did not materialize"
case "$worktree_path" in
    "$runtime_root_real"/worktrees/*) ;;
    *) fail "PR Agent worktree escaped the isolated runtime root" ;;
esac
[ -d "$worktree_path" ] || fail "recorded PR Agent worktree does not exist"
[ "$(git -C "$worktree_path" branch --show-current)" = "braid-agent/pr-$pull_number/$pr_profile-g1" ] || \
    fail "PR Agent worktree branch identity drifted"

for _ in $(seq 1 600); do
    if gh api "repos/$repository/contents/$fixture_path?ref=$head_ref" \
        --jq .content >"$temporary_root/content.b64" 2>/dev/null; then
        tr -d '\n' <"$temporary_root/content.b64" | base64 --decode >"$temporary_root/content.txt"
        [ "$(cat "$temporary_root/content.txt")" = "$fixture_marker" ] && break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited before the implementation diff"
    sleep 1
done
[ -f "$temporary_root/content.txt" ] && \
    [ "$(cat "$temporary_root/content.txt")" = "$fixture_marker" ] || \
    fail "PR Agent did not push the exact requested file"

files=$(gh api "repos/$repository/pulls/$pull_number/files" --jq 'map(.filename)')
printf '%s' "$files" | jq -e --arg path "$fixture_path" \
    'length == 1 and .[0] == $path' >/dev/null || fail "PR diff escaped the bounded fixture file"
pr_display=$("$braid" profile inspect --config "$config" --profile "$pr_profile" --json | jq -r .display_name)
agent_prefix=$(printf '> **Braid Agent · %s**\n> PR Implementation Agent' "$pr_display")
for _ in $(seq 1 120); do
    agent_comments=$(gh api "repos/$repository/issues/$pull_number/comments" --paginate | \
        jq -r --arg prefix "$agent_prefix" \
        '.[] | select(.body | startswith($prefix)) | .id' | wc -l | tr -d ' ')
    [ "$agent_comments" -ge 1 ] && break
    sleep 1
done
[ "${agent_comments:-0}" -ge 1 ] || fail "PR Implementation Agent published no attributed PR comment"
gh api "repos/$repository/issues/$pull_number/comments" --paginate --jq '.[].body' | \
    grep -q '# Braid Event References' && fail "Braid mirrored internal Event References to GitHub"

echo "PASS: Slice 5 write receipt + PR ensure + isolated PR Implementation Agent"
echo "candidate=$($braid --version)"
echo "repository=$repository issue=$issue_number pr=$pull_number comment=$comment_id receipt=$first_receipt"
echo "worktree=$worktree_path profile=$pr_profile diff=$fixture_path"
echo "scope=comment receipt, concurrent idempotency, bootstrap, Draft/native association, PR Profile/session/worktree, exact pushed diff, Agent-authored PR comment"
echo "UNPROVEN: review, cross-surface invalidation, origin variants, PR finalization/reopen/merge, full Slice 5"
