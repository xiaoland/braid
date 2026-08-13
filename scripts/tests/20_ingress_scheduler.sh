#!/bin/bash
set -euo pipefail

readonly script_name="$(basename "$0")"
readonly config_path="${BRAID_CONFIG:-}"
readonly binary="${BRAID_BIN:-$(command -v braid || true)}"
readonly keep_fixtures="${BRAID_TEST_KEEP_FIXTURES:-0}"
readonly webhook_mode="${BRAID_TEST_WEBHOOK_MODE:-app}"
readonly repository_webhook_url="${BRAID_TEST_PUBLIC_WEBHOOK_URL:-}"
readonly ingress_address="${BRAID_TEST_INGRESS:-127.0.0.1:18080}"
readonly health_address="${BRAID_TEST_HEALTH:-127.0.0.1:18081}"
readonly health_url="http://$health_address/healthz"

runtime_pid=""
temporary_root=""
fixture_issues=()
prior_webhook_url=""
repository_hook_id=""

fail() {
    printf '%s: FAIL: %s\n' "$script_name" "$*" >&2
    exit 1
}

note() {
    printf '%s: %s\n' "$script_name" "$*"
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command is unavailable: $1"
}

stop_runtime() {
    if [[ -n "$runtime_pid" ]] && kill -0 "$runtime_pid" 2>/dev/null; then
        kill -INT "$runtime_pid"
        wait "$runtime_pid" || true
    fi
    runtime_pid=""
}

cleanup() {
    local status=$?
    stop_runtime
    if [[ -n "$repository_hook_id" ]]; then
        gh api --method DELETE "repos/$repository/hooks/$repository_hook_id" >/dev/null 2>&1 || true
    fi
    if [[ "$keep_fixtures" != "1" ]]; then
        for issue in "${fixture_issues[@]:-}"; do
            gh issue close "$issue" --repo "$repository" --comment "Braid Slice 2 black-box fixture closed." >/dev/null 2>&1 || true
        done
    fi
    if [[ -n "$temporary_root" && -d "$temporary_root" ]]; then
        rm -rf "$temporary_root"
    fi
    if [[ $status -ne 0 ]]; then
        printf '%s: runtime log follows\n' "$script_name" >&2
        [[ -f "${runtime_log:-}" ]] && tail -200 "$runtime_log" >&2 || true
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

[[ -n "$binary" && -x "$binary" ]] || fail "set BRAID_BIN to the packaged braid binary"
[[ -n "$config_path" && "$config_path" = /* && -f "$config_path" ]] || \
    fail "set BRAID_CONFIG to an absolute acceptance configuration path"
[[ -n "${BRAID_WEBHOOK_SECRET:-}" ]] || \
    fail "BRAID_WEBHOOK_SECRET must match the dedicated GitHub App webhook secret"
[[ "$webhook_mode" == "app" || "$webhook_mode" == "repository" ]] || \
    fail "BRAID_TEST_WEBHOOK_MODE must be app or repository"
if [[ "$webhook_mode" == "repository" ]]; then
    [[ "$repository_webhook_url" == https://*.trycloudflare.com/webhook ]] || \
        fail "repository mode requires BRAID_TEST_PUBLIC_WEBHOOK_URL=https://<quick-tunnel>/webhook"
fi

for command in curl gh jq sed mktemp; do
    require_command "$command"
done

gh auth status >/dev/null 2>&1 || fail "gh must be authenticated as the Human fixture actor"
repository="$($binary config check --config "$config_path" --json | jq -er '.repository')"
app_actor="$($binary github probe --config "$config_path" --repository "$repository" --json | jq -er '.actor_login')"
if [[ "$webhook_mode" == "app" ]]; then
    prior_webhook_url="$($binary github webhook --config "$config_path" --json | jq -er '.url')"
fi

temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/braid-slice2.XXXXXX")"
readonly runtime_root="$temporary_root/runtime"
readonly runtime_log="$temporary_root/runtime.log"
readonly test_config="$temporary_root/braid.toml"
mkdir -p "$runtime_root/state/backups"

awk \
    -v root="$runtime_root" \
    -v database="$runtime_root/state/braid.sqlite3" \
    -v backups="$runtime_root/state/backups" \
    -v ingress="$ingress_address" \
    -v health="$health_address" '
    /^\[runtime\]$/ { in_runtime=1; print; next }
    /^\[server\]$/ { in_runtime=0; in_server=1; print; next }
    /^\[/ && $0 != "[runtime]" && $0 != "[server]" { in_runtime=0; in_server=0 }
    in_runtime && /^root = / { printf "root = \"%s\"\n", root; next }
    in_runtime && /^database = / { printf "database = \"%s\"\n", database; next }
    in_runtime && /^backups = / { printf "backups = \"%s\"\n", backups; next }
    in_runtime && /^auto_migrate = / { print "auto_migrate = false"; next }
    in_server && /^ingress = / { printf "ingress = \"%s\"\n", ingress; next }
    in_server && /^health = / { printf "health = \"%s\"\n", health; next }
    { print }
' "$config_path" > "$test_config"

$binary migrate apply --config "$test_config" >/dev/null

status_json() {
    "$binary" status --config "$test_config" --json
}

batch_json() {
    local issue_number=$1
    status_json | jq -ec --argjson number "$issue_number" \
        '.transport.batches[] | select(.work_item_kind == "issue" and .work_item_number == $number)'
}

wait_for_health() {
    local attempt expected_tunnel
    for attempt in $(seq 1 120); do
        expected_tunnel="connected"
        [[ "$webhook_mode" == "repository" ]] && expected_tunnel="disabled"
        if curl -fsS "$health_url" 2>/dev/null | \
            jq -e --arg tunnel "$expected_tunnel" '.ready == true and .tunnel == $tunnel' >/dev/null; then
            return 0
        fi
        if ! kill -0 "$runtime_pid" 2>/dev/null; then
            fail "runtime exited before becoming ready"
        fi
        sleep 1
    done
    fail "runtime did not become ready within 120 seconds"
}

start_runtime() {
    : > "$runtime_log"
    if [[ "$webhook_mode" == "app" ]]; then
        BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$binary" serve \
            --config "$test_config" --tunnel >"$runtime_log" 2>&1 &
    else
        BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$binary" serve \
            --config "$test_config" >"$runtime_log" 2>&1 &
    fi
    runtime_pid=$!
    wait_for_health
    if [[ "$webhook_mode" == "app" ]]; then
        local runtime_url app_url
        runtime_url="$(curl -fsS "$health_url" | jq -er '.webhook_url')"
        app_url="$($binary github webhook --config "$test_config" --json | jq -er '.url')"
        [[ "$runtime_url" == "$app_url" ]] || fail "runtime and GitHub App webhook URLs diverged"
    elif [[ -z "$repository_hook_id" ]]; then
        repository_hook_id="$(jq -cn \
            --arg url "$repository_webhook_url" \
            '{name:"web",active:true,events:["issues","issue_comment","pull_request","pull_request_review","pull_request_review_comment"],config:{url:$url,content_type:"json",insecure_ssl:"0",secret:env.BRAID_WEBHOOK_SECRET}}' | \
            gh api --method POST "repos/$repository/hooks" --input - --jq '.id')"
        [[ -n "$repository_hook_id" ]] || fail "GitHub did not create the temporary repository webhook"
    fi
}

wait_for_restoration() {
    [[ "$webhook_mode" == "repository" ]] && return 0
    local attempt current
    for attempt in $(seq 1 30); do
        current="$($binary github webhook --config "$test_config" --json | jq -er '.url')"
        [[ "$current" == "$prior_webhook_url" ]] && return 0
        sleep 1
    done
    fail "graceful shutdown did not restore the prior GitHub App webhook URL"
}

new_issue() {
    local purpose=$1 number
    number="$(gh api --method POST "repos/$repository/issues" \
        -f title="Braid Slice 2: $purpose $(date -u +%Y%m%dT%H%M%SZ)" \
        -f body="Disposable real-object fixture for the Braid Slice 2 ingress/scheduler gate." \
        --jq '.number')"
    printf '%s' "$number"
}

new_comment() {
    local issue_number=$1 body=$2
    gh api --method POST "repos/$repository/issues/$issue_number/comments" \
        -f body="$body" --jq '.id'
}

edit_comment() {
    local comment_id=$1 body=$2
    gh api --method PATCH "repos/$repository/issues/comments/$comment_id" \
        -f body="$body" >/dev/null
}

has_eyes() {
    local comment_id=$1
    gh api "repos/$repository/issues/comments/$comment_id/reactions" \
        -H "Accept: application/vnd.github+json" \
        --jq ".[] | select(.content == \"eyes\" and .user.login == \"$app_actor\") | .id" | \
        grep -q .
}

wait_for_eyes() {
    local comment_id=$1 attempt
    for attempt in $(seq 1 45); do
        has_eyes "$comment_id" && return 0
        sleep 1
    done
    fail "Braid App did not add eyes to comment $comment_id"
}

wait_for_batch() {
    local issue_number=$1 predicate=$2 attempt payload
    for attempt in $(seq 1 60); do
        payload="$(batch_json "$issue_number" 2>/dev/null || true)"
        if [[ -n "$payload" ]] && jq -e "$predicate" >/dev/null <<<"$payload"; then
            printf '%s' "$payload"
            return 0
        fi
        sleep 1
    done
    fail "batch for Issue #$issue_number did not satisfy: $predicate"
}

latest_delivery_id() {
    local after_id=$1 event=$2 action=$3 attempt id deliveries
    for attempt in $(seq 1 45); do
        if [[ "$webhook_mode" == "app" ]]; then
            deliveries="$($binary github deliveries --config "$test_config" --json)"
        else
            deliveries="$(gh api "repos/$repository/hooks/$repository_hook_id/deliveries?per_page=100")"
        fi
        id="$(jq -r --argjson after "$after_id" --arg event "$event" --arg action "$action" \
            '[.[] | select(.id > $after and .event == $event and .action == $action)] | max_by(.id) | .id // empty' \
            <<<"$deliveries")"
        [[ -n "$id" ]] && { printf '%s' "$id"; return 0; }
        sleep 1
    done
    fail "GitHub delivery $event/$action did not appear"
}

note "starting real Quick Tunnel and GitHub transport ($webhook_mode webhook mode)"
start_runtime

note "proving durable ingest, eyes, redelivery dedupe, and quiet-window reset"
basic_issue="$(new_issue "durable ingress")"
fixture_issues+=("$basic_issue")
if [[ "$webhook_mode" == "app" ]]; then
    before_delivery="$($binary github deliveries --config "$test_config" --json | jq '[.[].id] | max // 0')"
else
    before_delivery="$(gh api "repos/$repository/hooks/$repository_hook_id/deliveries?per_page=100" --jq '[.[].id] | max // 0')"
fi
first_comment="$(new_comment "$basic_issue" "first ordinary event")"
wait_for_eyes "$first_comment"
created_delivery="$(latest_delivery_id "$before_delivery" issue_comment created)"
first_batch="$(wait_for_batch "$basic_issue" '.lifecycle == "pending" and .event_count == 1 and .urgent == false')"
first_deadline="$(jq -r .quiet_deadline <<<"$first_batch")"
sleep 10
second_comment="$(new_comment "$basic_issue" "second ordinary event resets debounce")"
wait_for_eyes "$second_comment"
second_batch="$(wait_for_batch "$basic_issue" '.lifecycle == "pending" and .event_count >= 2')"
second_deadline="$(jq -r .quiet_deadline <<<"$second_batch")"
[[ "$second_deadline" > "$first_deadline" ]] || fail "quiet deadline did not move forward"

if [[ "$webhook_mode" == "app" ]]; then
    $binary github redeliver "$created_delivery" --config "$test_config" >/dev/null
else
    gh api --method POST "repos/$repository/hooks/$repository_hook_id/deliveries/$created_delivery/attempts" >/dev/null
fi
for _ in $(seq 1 30); do
    duplicates="$(status_json | jq -r '.transport.duplicate_deliveries')"
    [[ "$duplicates" -ge 1 ]] && break
    sleep 1
done
[[ "${duplicates:-0}" -ge 1 ]] || fail "redelivered GUID was not deduplicated"
after_redelivery="$(batch_json "$basic_issue")"
[[ "$(jq -r .event_count <<<"$after_redelivery")" == "$(jq -r .event_count <<<"$second_batch")" ]] || \
    fail "redelivery changed the logical batch"
sleep 22
[[ "$(batch_json "$basic_issue" | jq -r .lifecycle)" == "pending" ]] || \
    fail "batch released before 30 seconds from the second event"
wait_for_batch "$basic_issue" '.lifecycle == "runnable"' >/dev/null

note "proving the eight-event release threshold"
threshold_issue="$(new_issue "event threshold")"
fixture_issues+=("$threshold_issue")
for index in $(seq 1 8); do
    new_comment "$threshold_issue" "threshold event $index" >/dev/null
done
threshold_batch="$(wait_for_batch "$threshold_issue" '.lifecycle == "runnable" and .event_count >= 8')"
[[ "$(jq -r .urgent <<<"$threshold_batch")" == "false" ]] || fail "threshold batch was incorrectly urgent"

note "proving visible trusted mention grammar"
grammar_issue="$(new_issue "mention grammar")"
fixture_issues+=("$grammar_issue")
new_comment "$grammar_issue" '> @braid quoted text must not be urgent' >/dev/null
new_comment "$grammar_issue" '`@braid` inline code must not be urgent' >/dev/null
new_comment "$grammar_issue" '<!-- @braid hidden text must not be urgent -->' >/dev/null
grammar_batch="$(wait_for_batch "$grammar_issue" '.event_count >= 3')"
[[ "$(jq -r .urgent <<<"$grammar_batch")" == "false" ]] || fail "hidden/code/quoted mention became urgent"

urgent_issue="$(new_issue "trusted mention")"
fixture_issues+=("$urgent_issue")
urgent_comment="$(new_comment "$urgent_issue" '@braid please inspect this fixture')"
wait_for_eyes "$urgent_comment"
wait_for_batch "$urgent_issue" '.lifecycle == "runnable" and .urgent == true' >/dev/null

note "proving pending-batch restart and reconciliation across tunnel loss"
restart_issue="$(new_issue "restart and reconciliation")"
fixture_issues+=("$restart_issue")
restart_comment="$(new_comment "$restart_issue" 'pending before restart')"
wait_for_eyes "$restart_comment"
wait_for_batch "$restart_issue" '.lifecycle == "pending" and .event_count == 1' >/dev/null
stop_runtime
wait_for_restoration

if [[ "$webhook_mode" == "app" ]]; then
    before_lost_delivery="$($binary github deliveries --config "$test_config" --json | jq '[.[].id] | max // 0')"
else
    before_lost_delivery="$(gh api "repos/$repository/hooks/$repository_hook_id/deliveries?per_page=100" --jq '[.[].id] | max // 0')"
fi
lost_comment="$(new_comment "$restart_issue" 'created while the local tunnel is unavailable')"
lost_delivery="$(latest_delivery_id "$before_lost_delivery" issue_comment created)"

start_runtime
wait_for_eyes "$lost_comment"
wait_for_batch "$restart_issue" '.event_count >= 2' >/dev/null
edit_comment "$lost_comment" 'newer canonical body after runtime recovery'
sleep 3
count_before_old="$(batch_json "$restart_issue" | jq -r .event_count)"
if [[ "$webhook_mode" == "app" ]]; then
    $binary github redeliver "$lost_delivery" --config "$test_config" >/dev/null
else
    gh api --method POST "repos/$repository/hooks/$repository_hook_id/deliveries/$lost_delivery/attempts" >/dev/null
fi
sleep 5
count_after_old="$(batch_json "$restart_issue" | jq -r .event_count)"
[[ "$count_after_old" == "$count_before_old" ]] || \
    fail "an older delivery regressed or duplicated the canonical batch"

stop_runtime
wait_for_restoration

jq -n \
    --arg repository "$repository" \
    --arg app_actor "$app_actor" \
    --arg webhook_mode "$webhook_mode" \
    --argjson durable_issue "$basic_issue" \
    --argjson threshold_issue "$threshold_issue" \
    --argjson grammar_issue "$grammar_issue" \
    --argjson urgent_issue "$urgent_issue" \
    --argjson restart_issue "$restart_issue" \
    '{
        verdict:"PASS",
        boundary:"real GitHub webhook -> Quick Tunnel -> packaged Braid -> Braid App reactions/status",
        webhook_mode:$webhook_mode,
        repository:$repository,
        app_actor:$app_actor,
        fixtures:[$durable_issue,$threshold_issue,$grammar_issue,$urgent_issue,$restart_issue]
    }'
