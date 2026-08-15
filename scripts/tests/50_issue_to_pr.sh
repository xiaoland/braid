#!/bin/sh
set -eu

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

require_comment_result() {
    result=$1
    if ! printf '%s' "$result" | jq -e \
        '.state == "applied" and (.comment | type == "string") and
         (.comment | contains("#issuecomment-"))' >/dev/null; then
        printf 'Unexpected braid gh comment result:\n%s\n' "$result" >&2
        fail "comment result omitted its semantic GitHub comment reference"
    fi
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
restart_issue_number=
pull_number=
head_ref=
merge_base_ref=
runtime_pid=
tunnel_pid=
repository_hook_id=
worktree_path=
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-slice5.XXXXXX")
runtime_root="$temporary_root/runtime"
source_checkout="$temporary_root/source"
config="$temporary_root/braid.toml"
runtime_log="$temporary_root/runtime.log"
tunnel_log="$temporary_root/tunnel.log"

latest_pr_group() {
    "$braid" status --config "$config" --json | jq -c \
        --argjson number "$pull_number" --arg profile "$pr_profile" \
        '[.transport.agent_groups[]? | select(
          .work_item_kind == "pr" and .work_item_number == $number and
          .profile_id == $profile)][-1] // empty'
}

latest_issue_group() {
    "$braid" status --config "$config" --json | jq -c \
        --argjson number "$restart_issue_number" --arg profile "$issue_profile" \
        '[.transport.agent_groups[]? | select(
          .work_item_kind == "issue" and .work_item_number == $number and
          .profile_id == $profile)][-1] // empty'
}

duplicate_agent_attribution_count() {
    gh api "repos/$repository/issues/$pull_number/comments" --paginate | jq \
        --arg prefix "$agent_prefix" \
        '[.[] | select(.body | startswith($prefix + "\n\n" + $prefix))] | length'
}

review_reaction_exists() {
    comment_id=$1
    content=$2
    gh api "repos/$repository/pulls/comments/$comment_id/reactions" \
        --jq "any(.content == \"$content\" and .user.login == \"$app_actor\")" \
        2>/dev/null | grep -q true
}

issue_reaction_exists() {
    comment_id=$1
    content=$2
    gh api "repos/$repository/issues/comments/$comment_id/reactions" \
        --jq "any(.content == \"$content\" and .user.login == \"$app_actor\")" \
        2>/dev/null | grep -q true
}

stop_process() {
    pid=$1
    [ -n "$pid" ] || return 0
    kill -0 "$pid" 2>/dev/null || return 0
    kill -INT "$pid" 2>/dev/null || true
    for _ in $(seq 1 15); do
        kill -0 "$pid" 2>/dev/null || {
            wait "$pid" 2>/dev/null || true
            return 0
        }
        sleep 1
    done
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    stop_process "$runtime_pid"
    stop_process "$tunnel_pid"
    if [ -n "$repository_hook_id" ]; then
        gh api --method DELETE "repos/$repository/hooks/$repository_hook_id" >/dev/null 2>&1 || true
    fi
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
    if [ -n "$merge_base_ref" ]; then
        gh api --method DELETE "repos/$repository/git/refs/heads/$merge_base_ref" >/dev/null 2>&1 || true
    fi
    if [ -n "$issue_number" ]; then
        gh issue close "$issue_number" --repo "$repository" --reason "not planned" >/dev/null 2>&1 || true
    fi
    if [ -n "$restart_issue_number" ]; then
        gh issue close "$restart_issue_number" --repo "$repository" --reason "not planned" >/dev/null 2>&1 || true
    fi
    if [ "$status" -ne 0 ]; then
        echo "Slice 5 runtime log follows" >&2
        [ -f "$runtime_log" ] && tail -200 "$runtime_log" >&2 || true
        echo "Slice 5 tunnel log follows" >&2
        [ -f "$tunnel_log" ] && tail -100 "$tunnel_log" >&2 || true
    fi
    rm -rf "$temporary_root"
}
trap cleanup EXIT HUP INT TERM

command -v "$braid" >/dev/null 2>&1 || [ -x "$braid" ] || fail "Braid binary is unavailable"
command -v gh >/dev/null 2>&1 || fail "gh is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"
command -v curl >/dev/null 2>&1 || fail "curl is unavailable"
command -v git >/dev/null 2>&1 || fail "git is unavailable"
wrangler=${BRAID_TEST_WRANGLER:-$(command -v wrangler || true)}
[ -n "$wrangler" ] && [ -x "$wrangler" ] || fail "set BRAID_TEST_WRANGLER to Wrangler"
[ -n "${BRAID_WEBHOOK_SECRET:-}" ] || fail "BRAID_WEBHOOK_SECRET is required by the runtime"
[ "$source_config" = "${source_config#/}" ] && fail "BRAID_CONFIG must be absolute"
[ -f "$source_config" ] || fail "BRAID_CONFIG does not exist"
braid_path=$(command -v "$braid" 2>/dev/null || printf '%s' "$braid")

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
    /^github_actor_node_id = / { next }
    /^\[runtime\]$/ { section="runtime"; print; next }
    /^\[server\]$/ { section="server"; print; next }
    /^\[scheduler\]$/ { section="scheduler"; print; next }
    /^\[\[profiles\]\]$/ { section="profile"; profile=""; print; next }
    /^\[/ && $0 != "[runtime]" && $0 != "[server]" && $0 != "[scheduler]" && $0 != "[[profiles]]" { section="other" }
    section == "runtime" && /^root = / { printf "root = \"%s\"\n", root; next }
    section == "runtime" && /^database = / { printf "database = \"%s\"\n", database; next }
    section == "runtime" && /^backups = / { printf "backups = \"%s\"\n", backups; next }
    section == "runtime" && /^auto_migrate = / { print "auto_migrate = false"; next }
    section == "server" && /^ingress = / { printf "ingress = \"%s\"\n", ingress; next }
    section == "server" && /^health = / { printf "health = \"%s\"\n", health; next }
    section == "scheduler" && /^quiet_seconds = / { print "quiet_seconds = 5"; next }
    section == "scheduler" && /^reconciliation_seconds = / { print "reconciliation_seconds = 60"; next }
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
app_actor=$(printf '%s' "$identity" | jq -er .actor_login)
jq -e '.permissions.issues == "write" and .permissions.pull_requests == "write" and .permissions.contents == "write"' \
    >/dev/null <<EOF || fail "Braid App requires Issues, Pull requests, and Contents write"
$identity
EOF

binary_directory=$(dirname "$braid")

start_runtime() {
    PATH="$binary_directory:$PATH" BRAID_CONFIG="$config" \
        BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" \
        "$braid" serve --config "$config" >"$runtime_log" 2>&1 &
    runtime_pid=$!
    for _ in $(seq 1 120); do
        if curl -fsS "$health_url" 2>/dev/null | \
            jq -e '.ready == true and .provider == "connected"' >/dev/null; then
            return 0
        fi
        kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during PR Agent startup"
        sleep 1
    done
    fail "Braid did not become provider-ready"
}

TUNNEL_TRANSPORT_PROTOCOL=http2 "$wrangler" tunnel quick-start \
    "http://$ingress_address" >"$tunnel_log" 2>&1 &
tunnel_pid=$!
for _ in $(seq 1 90); do
    tunnel_url=$(grep -Eo 'https://[a-z0-9-]+\.trycloudflare\.com' "$tunnel_log" | tail -1 || true)
    [ -n "$tunnel_url" ] && break
    kill -0 "$tunnel_pid" 2>/dev/null || fail "Quick Tunnel exited before publishing a URL"
    sleep 1
done
[ -n "${tunnel_url:-}" ] || fail "Quick Tunnel did not publish a URL"

start_runtime

public_probe_ready=0
for _ in $(seq 1 18); do
    if BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$braid" tunnel probe \
        --config "$config" --url "$tunnel_url/webhook" >/dev/null 2>&1; then
        public_probe_ready=1
        break
    fi
    sleep 5
done
[ "$public_probe_ready" -eq 1 ] || fail "public signed webhook probe failed"

repository_hook_id=$(gh api --method POST "repos/$repository/hooks" \
    -f name=web \
    -F active=true \
    -f 'events[]=issues' \
    -f 'events[]=issue_comment' \
    -f 'events[]=pull_request' \
    -f 'events[]=pull_request_review' \
    -f 'events[]=pull_request_review_comment' \
    -f 'events[]=pull_request_review_thread' \
    -f "config[url]=$tunnel_url/webhook" \
    -f 'config[content_type]=json' \
    -f "config[secret]=$BRAID_WEBHOOK_SECRET" \
    --jq .id)

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
require_comment_result "$comment_json"
comment_id=$(printf '%s' "$comment_json" | jq -r '.comment | split("#issuecomment-")[1]')
profile_display=$("$braid" profile inspect --config "$config" --profile "$issue_profile" --json | jq -r .display_name)
[ -n "$comment_id" ] && [ "$comment_id" != null ] || fail "comment result omitted its GitHub comment reference"
body=$(gh api "repos/$repository/issues/comments/$comment_id" --jq .body)
first=$(printf '%s\n' "$body" | sed -n '1p')
second=$(printf '%s\n' "$body" | sed -n '2p')
[ "$first" = "> **Braid Agent · $profile_display**" ] || fail "Braid attribution display name drifted"
[ "$second" = '> Issue Agent' ] || fail "Braid attribution role drifted"
comment_retry_json=$("$braid" gh comment create "$repository#$issue_number" \
    --config "$config" \
    --profile "$issue_profile" \
    --request-id "slice5-$stamp-implementation-request" \
    --body "Implementation Request: implement the exact bounded file change in this Issue description, verify it, push it, and report concisely on the Draft PR." \
    --json)
require_comment_result "$comment_retry_json"
retry_comment_id=$(printf '%s' "$comment_retry_json" | jq -r '.comment | split("#issuecomment-")[1]')
[ "$retry_comment_id" = "$comment_id" ] || fail "idempotent comment retry created a different comment"
printf '%s' "$comment_retry_json" | jq -e \
    '(has("intent_id") | not) and (has("request_digest") | not) and (has("remote_node_id") | not) and (has("write") | not)' \
    >/dev/null || fail "comment result leaked internal identifiers"

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

first_pr=$(jq -r '.pull_request | split("#")[1]' "$temporary_root/ensure-a.json")
second_pr=$(jq -r '.pull_request | split("#")[1]' "$temporary_root/ensure-b.json")
[ "$first_pr" = "$second_pr" ] || fail "concurrent pr ensure calls returned different PRs"
pull_number=$first_pr
[ "$pull_number" != null ] || fail "pr ensure result omitted its GitHub PR reference"
jq -e \
    '(has("write") | not) and (has("intent_id") | not) and (has("request_digest") | not) and (has("pull_request_node_id") | not)' \
    "$temporary_root/ensure-a.json" >/dev/null || fail "PR result leaked internal identifiers"

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

for _ in $(seq 1 60); do
    files=$(gh api "repos/$repository/pulls/$pull_number/files" --jq 'map(.filename)')
    if printf '%s' "$files" | jq -e --arg path "$fixture_path" \
        'length == 1 and .[0] == $path' >/dev/null; then
        break
    fi
    sleep 1
done
printf '%s' "$files" | jq -e --arg path "$fixture_path" \
    'length == 1 and .[0] == $path' >/dev/null || fail "PR diff escaped the bounded fixture file"
pr_display=$("$braid" profile inspect --config "$config" --profile "$pr_profile" --json | jq -r .display_name)
agent_prefix=$(printf '> **Braid Agent · %s**\n> PR Implementation Agent' "$pr_display")
gh api "repos/$repository/issues/$pull_number/comments" --paginate --jq '.[].body' | \
    grep -q '# Braid Event References' && fail "Braid mirrored internal Event References to GitHub"

initial_group=$(latest_pr_group)
initial_session=$(printf '%s' "$initial_group" | jq -er .provider_session_id)
initial_worktree=$(printf '%s' "$initial_group" | jq -er .worktree_path)

review_hidden_marker="BRAID_SLICE5_FOLDED_REVIEW_$stamp"
head_sha=$(gh pr view "$pull_number" --repo "$repository" --json headRefOid --jq .headRefOid)
review_comment=$(gh api --method POST "repos/$repository/pulls/$pull_number/comments" \
    -f body="Review acceptance $review_hidden_marker: inspect this thread and decide whether any response is useful. Do not modify the implementation." \
    -f commit_id="$head_sha" \
    -f path="$fixture_path" \
    -F line=1 \
    -f side=RIGHT)
review_comment_id=$(printf '%s' "$review_comment" | jq -er .id)
review_comment_node_id=$(printf '%s' "$review_comment" | jq -er .node_id)
review_baseline_turns=$(latest_pr_group | jq -er .turn_count)

for _ in $(seq 1 240); do
    review_turns=$(latest_pr_group | jq -er .turn_count)
    if review_reaction_exists "$review_comment_id" eyes && \
        [ "$review_turns" -gt "$review_baseline_turns" ]; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited while handling PR review feedback"
    sleep 1
done
review_reaction_exists "$review_comment_id" eyes || fail "PR review comment was not durably acknowledged"
[ "$review_turns" -gt "$review_baseline_turns" ] || \
    fail "PR review comment did not Wake the PR Agent"
review_group=$(latest_pr_group)
review_pre_reset_session=$(printf '%s' "$review_group" | jq -er .provider_session_id)

review_thread_id=$(gh api graphql \
    -f query='query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewThreads(first:100){nodes{id isResolved comments(first:100){nodes{id}}}}}}}' \
    -f owner="${repository%%/*}" \
    -f name="${repository#*/}" \
    -F number="$pull_number" \
    --jq ".data.repository.pullRequest.reviewThreads.nodes[] | select(any(.comments.nodes[]; .id == \"$review_comment_node_id\")) | .id")
[ -n "$review_thread_id" ] || fail "could not resolve the created review comment to a review thread"

gh api graphql \
    -f query='mutation($thread:ID!){resolveReviewThread(input:{threadId:$thread}){thread{id isResolved}}}' \
    -f thread="$review_thread_id" | \
    jq -e '.data.resolveReviewThread.thread.isResolved == true' >/dev/null || \
    fail "GitHub did not resolve the review thread"

review_reset_session=
stable_review_session=
stable_review_observations=0
for _ in $(seq 1 300); do
    review_group=$(latest_pr_group)
    if printf '%s' "$review_group" | jq -e \
        --arg old "$review_pre_reset_session" '
        .provider_session_id != $old and .session_lifecycle == "idle" and
        .active_turn_id == null' >/dev/null; then
        candidate_session=$(printf '%s' "$review_group" | jq -er .provider_session_id)
        if [ "$candidate_session" = "$stable_review_session" ]; then
            stable_review_observations=$((stable_review_observations + 1))
        else
            stable_review_session=$candidate_session
            stable_review_observations=1
        fi
        if [ "$stable_review_observations" -ge 5 ]; then
            review_reset_session=$candidate_session
            break
        fi
    else
        stable_review_session=
        stable_review_observations=0
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited before review replacement converged"
    sleep 1
done
[ -n "$review_reset_session" ] || fail "resolved review thread did not converge to current PR Context"
[ "$(printf '%s' "$review_group" | jq -r .worktree_path)" = "$initial_worktree" ] || \
    fail "review Context replacement changed the dedicated worktree"
"$braid" context pr "$repository#$pull_number" --config "$config" >"$temporary_root/review-context.md"
grep -q "Review thread at $fixture_path:1" "$temporary_root/review-context.md" || \
    fail "resolved review thread location is absent from current PR Context"
grep -q 'State: resolved' "$temporary_root/review-context.md" || \
    fail "review thread did not render its resolved lifecycle"
grep -q "$review_hidden_marker" "$temporary_root/review-context.md" && \
    fail "resolved review-thread body remained in current PR Context"

cross_marker="BRAID_SLICE5_DESIGN_$stamp"
active_marker="BRAID_SLICE5_ACTIVE_$stamp"
active_comment=$(gh api --method POST "repos/$repository/issues/$pull_number/comments" \
    -f body="@braid Acceptance: first run \`sleep 30\` in the dedicated worktree so this turn remains active, then publish one concise attributed PR comment containing $active_marker." --jq .id)

active_session=
for _ in $(seq 1 120); do
    active_group=$(latest_pr_group)
    if [ -n "$active_group" ] && printf '%s' "$active_group" | \
        jq -e '.session_lifecycle == "running" and .active_turn_id != null' >/dev/null; then
        active_session=$(printf '%s' "$active_group" | jq -er .provider_session_id)
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited before the active PR turn"
    sleep 1
done
[ -n "$active_session" ] || fail "trusted PR mention did not start an observable active turn"

updated_issue_body="Disposable real-object fixture. The implementation remains the exact file requested below. Current accepted design marker: $cross_marker. Implement by creating \`$fixture_path\` containing exactly \`$fixture_marker\`, then commit, push the PR branch, verify the diff, and publish concise PR comments."
gh issue edit "$issue_number" --repo "$repository" --body "$updated_issue_body" >/dev/null
edit_epoch=$(date +%s)
sleep 3
status_payload=$("$braid" status --config "$config" --json)
early_cross_reset=$(printf '%s' "$status_payload" | jq \
    --argjson number "$pull_number" --arg old "$active_session" '
    [.transport.context_resets[]? | select(
      .work_item_kind == "pr" and .work_item_number == $number and
      .old_provider_session_id == $old and .lifecycle == "applied")] | length')
[ "$early_cross_reset" -eq 0 ] || fail "Associated-Issue invalidation bypassed the configured debounce window"

cross_reset_session=
cross_reset_continuation=
for _ in $(seq 1 240); do
    status_payload=$("$braid" status --config "$config" --json)
    cross_reset=$(printf '%s' "$status_payload" | jq -c \
        --argjson number "$pull_number" --arg old "$active_session" '
        [.transport.context_resets[]? | select(
          .work_item_kind == "pr" and .work_item_number == $number and
          .old_provider_session_id == $old and .lifecycle == "applied")][-1] // empty')
    if [ -n "$cross_reset" ]; then
        cross_reset_session=$(printf '%s' "$cross_reset" | jq -er .new_provider_session_id)
        cross_reset_continuation=$(printf '%s' "$cross_reset" | jq -er .continuation)
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during cross-surface Context replacement"
    sleep 1
done
[ -n "$cross_reset_session" ] || fail "Associated-Issue description edit did not replace PR Context"
[ "$cross_reset_continuation" = true ] || fail "active cross-surface invalidation did not schedule one continuation"
elapsed=$(( $(date +%s) - edit_epoch ))
[ "$elapsed" -ge 5 ] || fail "cross-surface invalidation completed before the debounce window"

for _ in $(seq 1 300); do
    cross_group=$(latest_pr_group)
    if [ -n "$cross_group" ] && printf '%s' "$cross_group" | jq -e \
        --arg session "$cross_reset_session" '
        .provider_session_id == $session and .session_lifecycle == "idle" and
        .active_turn_id == null' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited before the replacement PR turn converged"
    sleep 1
done
if [ "$(printf '%s' "$cross_group" | jq -r .provider_session_id)" != "$cross_reset_session" ]; then
    echo "expected cross-surface session: $cross_reset_session" >&2
    printf '%s' "$status_payload" | jq \
        --argjson number "$pull_number" --arg profile "$pr_profile" \
        '{groups:[.transport.agent_groups[]? | select(
          .work_item_kind == "pr" and .work_item_number == $number and
          .profile_id == $profile)], resets:[.transport.context_resets[]? | select(
          .work_item_kind == "pr" and .work_item_number == $number and
          .profile_id == $profile)]}' >&2
    fail "PR Agent did not adopt the cross-surface replacement session"
fi
[ "$(printf '%s' "$cross_group" | jq -r .worktree_path)" = "$initial_worktree" ] || \
    fail "cross-surface Context replacement changed the dedicated worktree"
"$braid" context pr "$repository#$pull_number" --config "$config" >"$temporary_root/cross-context.md"
grep -q "$cross_marker" "$temporary_root/cross-context.md" || \
    fail "replacement PR Context omitted the edited Associated-Issue description"

active_reactions=$(gh api "repos/$repository/issues/comments/$active_comment/reactions" --jq 'map(.content)')
printf '%s' "$active_reactions" | jq -e 'all(.[]; . != "rocket" and . != "+1" and . != "confused")' \
    >/dev/null || fail "superseded trusted turn retained a terminal lifecycle reaction"
gh api "repos/$repository/issues/$pull_number/comments" --paginate --jq '.[].body' | \
    grep -q '# Braid Event References' && fail "Braid mirrored Event References after replacement"
[ "$(duplicate_agent_attribution_count)" -eq 0 ] || \
    fail "an Agent comment contains duplicate generated attribution"

direct_external_marker="BRAID_SLICE5_DIRECT_EXTERNAL_$stamp"
direct_baseline_turns=$(latest_pr_group | jq -er .turn_count)
direct_external_comment=$(gh api --method POST "repos/$repository/issues/$pull_number/comments" \
    -f body="Direct gh acceptance $direct_external_marker: inspect this external comment and decide whether any response is useful. Do not modify the implementation." \
    --jq .id)
for _ in $(seq 1 240); do
    direct_group=$(latest_pr_group)
    direct_turns=$(printf '%s' "$direct_group" | jq -er .turn_count)
    if issue_reaction_exists "$direct_external_comment" eyes && \
        [ "$direct_turns" -gt "$direct_baseline_turns" ]; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during unconfigured direct-gh origin"
    sleep 1
done
issue_reaction_exists "$direct_external_comment" eyes || \
    fail "unconfigured direct gh comment was not treated as external"
[ "$direct_turns" -gt "$direct_baseline_turns" ] || \
    fail "unconfigured direct gh comment did not create a turn"

preclose_group=$(latest_pr_group)
preclose_session=$(printf '%s' "$preclose_group" | jq -er .provider_session_id)
preclose_turns=$(printf '%s' "$preclose_group" | jq -er .turn_count)
gh pr close "$pull_number" --repo "$repository" >/dev/null
for _ in $(seq 1 300); do
    closed_group=$(latest_pr_group)
    if printf '%s' "$closed_group" | jq -e \
        '.assignment_lifecycle == "sleeping" and .session_lifecycle == "sleeping" and
         .worktree_lifecycle == "sleeping" and .finalization_turns == 1 and
         .last_finalization_lifecycle == "completed"' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during PR close finalization"
    sleep 1
done
printf '%s' "$closed_group" | jq -e \
    '.assignment_lifecycle == "sleeping" and .finalization_turns == 1' >/dev/null || \
    fail "closed PR did not converge through exactly one Finalization Turn"
[ "$(printf '%s' "$closed_group" | jq -er .turn_count)" -eq $((preclose_turns + 1)) ] || \
    fail "PR close produced an unexpected number of turns"

closed_comment=$(gh api --method POST "repos/$repository/issues/$pull_number/comments" \
    -f body="Closed PR sleep acceptance: acknowledge this comment but do not start another turn." \
    --jq .id)
for _ in $(seq 1 30); do
    issue_reaction_exists "$closed_comment" eyes && break
    sleep 1
done
issue_reaction_exists "$closed_comment" eyes || fail "closed PR comment was not durably seen"
sleep 8
closed_after=$(latest_pr_group)
[ "$(printf '%s' "$closed_after" | jq -er .turn_count)" -eq $((preclose_turns + 1)) ] || \
    fail "closed PR event granted a second Finalization Turn"

gh pr reopen "$pull_number" --repo "$repository" >/dev/null
for _ in $(seq 1 300); do
    reopened_group=$(latest_pr_group)
    if printf '%s' "$reopened_group" | jq -e \
        --arg old "$preclose_session" --argjson minimum $((preclose_turns + 2)) '
        .assignment_lifecycle == "active" and .session_lifecycle == "idle" and
        .worktree_lifecycle == "active" and .provider_session_id != $old and
        .turn_count >= $minimum and .active_turn_id == null' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during PR reopen"
    sleep 1
done
printf '%s' "$reopened_group" | jq -e \
    '.assignment_lifecycle == "active" and .worktree_lifecycle == "active"' >/dev/null || \
    fail "reopened PR did not restore its Agent Group/worktree"
[ "$(printf '%s' "$reopened_group" | jq -er .worktree_path)" = "$initial_worktree" ] || \
    fail "PR reopen replaced the dedicated worktree"

reopened_session=$(printf '%s' "$reopened_group" | jq -er .provider_session_id)
reopened_resume_count=$(printf '%s' "$reopened_group" | jq -er .provider_resume_count)
stop_process "$runtime_pid"
runtime_pid=
start_runtime
for _ in $(seq 1 180); do
    resumed_group=$(latest_pr_group)
    if printf '%s' "$resumed_group" | jq -e \
        --arg session "$reopened_session" \
        --arg worktree "$initial_worktree" \
        --argjson resumes $((reopened_resume_count + 1)) '
        .assignment_lifecycle == "active" and .session_lifecycle == "idle" and
        .provider_session_id == $session and .worktree_path == $worktree and
        .worktree_lifecycle == "active" and .provider_resume_count >= $resumes' \
        >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited while resuming the reopened PR Agent"
    sleep 1
done
printf '%s' "$resumed_group" | jq -e \
    --arg session "$reopened_session" --arg worktree "$initial_worktree" '
    .provider_session_id == $session and .worktree_path == $worktree and
    .session_lifecycle == "idle"' >/dev/null || \
    fail "Braid restart did not resume the same PR provider session and worktree"

merge_base_ref="acceptance/slice5-base-$stamp"
gh api --method POST "repos/$repository/git/refs" \
    -f ref="refs/heads/$merge_base_ref" -f sha="$parent_sha" >/dev/null
gh pr edit "$pull_number" --repo "$repository" --base "$merge_base_ref" >/dev/null
for _ in $(seq 1 240); do
    before_merge_group=$(latest_pr_group)
    if printf '%s' "$before_merge_group" | jq -e \
        '.assignment_lifecycle == "active" and .session_lifecycle == "idle" and
         .active_turn_id == null' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited while changing the disposable merge base"
    sleep 1
done
premerge_turns=$(printf '%s' "$before_merge_group" | jq -er .turn_count)
gh pr ready "$pull_number" --repo "$repository" >/dev/null
gh pr merge "$pull_number" --repo "$repository" --merge --delete-branch=false >/dev/null
for _ in $(seq 1 300); do
    merged_group=$(latest_pr_group)
    if printf '%s' "$merged_group" | jq -e \
        --argjson minimum $((premerge_turns + 1)) '
        .assignment_lifecycle == "retired" and .session_lifecycle == "retired" and
        .worktree_lifecycle == "retired" and .finalization_turns == 2 and
        .last_finalization_lifecycle == "completed" and .turn_count == $minimum' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during merged PR finalization"
    sleep 1
done
printf '%s' "$merged_group" | jq -e \
    '.assignment_lifecycle == "retired" and .finalization_turns == 2' >/dev/null || \
    fail "merged PR did not retire after exactly one final Finalization Turn"
gh pr view "$pull_number" --repo "$repository" --json state,mergedAt,baseRefName | jq -e \
    --arg base "$merge_base_ref" '.state == "MERGED" and .mergedAt != null and .baseRefName == $base' \
    >/dev/null || fail "PR was not merged only into the disposable base"

restart_issue_url=$(gh issue create \
    --repo "$repository" \
    --title "Braid Slice 6 active restart $stamp" \
    --body "Disposable active-turn restart fixture. Inspect the repository documentation carefully before responding; make no repository changes.")
restart_issue_number=${restart_issue_url##*/}
restart_comment=$(gh api --method POST "repos/$repository/issues/$restart_issue_number/comments" \
    -f body="@braid Inspect the repository documentation carefully before responding. Keep this turn read-only and report concisely." \
    --jq .id)
for _ in $(seq 1 120); do
    active_restart_group=$(latest_issue_group)
    if printf '%s' "$active_restart_group" | jq -e \
        '.assignment_lifecycle == "active" and .session_lifecycle == "running" and
         .active_turn_id != null and .turn_lifecycle == "running"' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited before the restart fixture became active"
    sleep 1
done
printf '%s' "$active_restart_group" | jq -e '.active_turn_id != null' >/dev/null || \
    fail "trusted mention did not create an active Issue turn for restart"
restart_session=$(printf '%s' "$active_restart_group" | jq -er .provider_session_id)
restart_turns=$(printf '%s' "$active_restart_group" | jq -er .turn_count)
issue_reaction_exists "$restart_comment" rocket || \
    fail "active restart fixture did not expose the trusted-mention rocket"
stop_process "$runtime_pid"
runtime_pid=
start_runtime
for _ in $(seq 1 120); do
    unknown_group=$(latest_issue_group)
    unknown_status_count=$(gh api "repos/$repository/issues/$restart_issue_number/comments" --paginate | jq \
        --arg app "$app_actor" \
        '[.[] | select(.user.login == $app and (.body | contains("Provider outcome unknown")))] | length')
    if printf '%s' "$unknown_group" | jq -e \
        --arg session "$restart_session" --argjson turns "$restart_turns" '
        .provider_session_id == $session and .session_lifecycle == "unknown" and
        .turn_lifecycle == "unknown" and
        .turn_count == $turns and .provider_resume_count >= 1' >/dev/null && \
        [ "$unknown_status_count" -eq 1 ]; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited while converging an unknown active turn"
    sleep 1
done
printf '%s' "$unknown_group" | jq -e \
    --arg session "$restart_session" --argjson turns "$restart_turns" '
    .provider_session_id == $session and .session_lifecycle == "unknown" and
    .turn_lifecycle == "unknown" and .turn_count == $turns' >/dev/null || \
    fail "active-turn restart did not preserve one neutral unknown turn on the same provider session"
[ "$unknown_status_count" -eq 1 ] || fail "active-turn restart did not converge one Operational Status Comment"
issue_reaction_exists "$restart_comment" rocket || \
    fail "unknown active-turn restart incorrectly claimed a terminal reaction"

echo "PASS: Slice 5 Issue-to-PR, direct origin, review/reset, and PR lifecycle"
echo "candidate=$($braid --version)"
echo "repository=$repository issue=$issue_number pr=$pull_number comment=$comment_id"
echo "profile=$pr_profile diff=$fixture_path worktree=preserved"
echo "scope=semantic comment result, concurrent idempotency, bootstrap, Draft/native association, PR Profile/session/worktree, exact diff, review wake, review-thread invalidation, debounced Associated-Issue active invalidation, unconfigured direct gh origin, close/sleep/reopen, idle PR session/worktree restart, merge/retire, active Issue turn restart to one neutral unknown"
echo "UNPROVEN: configured stable direct-gh actor, full Slice 5 campaign matrix, and native App assignment capability"
