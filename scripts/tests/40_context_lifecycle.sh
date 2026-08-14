#!/bin/bash
set -euo pipefail

readonly script_name="$(basename "$0")"
readonly config_path="${BRAID_CONFIG:-}"
readonly binary="${BRAID_BIN:-$(command -v braid || true)}"
readonly ingress_address="${BRAID_TEST_INGRESS:-127.0.0.1:18080}"
readonly health_address="${BRAID_TEST_HEALTH:-127.0.0.1:18081}"
readonly health_url="http://$health_address/healthz"
readonly keep_fixture="${BRAID_TEST_KEEP_FIXTURES:-0}"

runtime_pid=""
tunnel_pid=""
repository_hook_id=""
fixture_issue=""
soft_issue=""
hard_issue=""
temporary_root=""
runtime_log=""
tunnel_log=""

fail() {
    printf '%s: FAIL: %s\n' "$script_name" "$*" >&2
    exit 1
}

note() {
    printf '%s: %s\n' "$script_name" "$*"
}

stop_process() {
    local pid=$1
    [[ -n "$pid" ]] || return 0
    kill -0 "$pid" 2>/dev/null || return 0
    kill -INT "$pid" 2>/dev/null || true
    for _ in $(seq 1 10); do
        kill -0 "$pid" 2>/dev/null || { wait "$pid" 2>/dev/null || true; return 0; }
        sleep 1
    done
    kill -TERM "$pid" 2>/dev/null || true
    sleep 2
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    local exit_status=$?
    stop_process "$runtime_pid"
    stop_process "$tunnel_pid"
    if [[ -n "$repository_hook_id" ]]; then
        gh api --method DELETE "repos/$repository/hooks/$repository_hook_id" >/dev/null 2>&1 || true
    fi
    if [[ "$keep_fixture" != "1" ]]; then
        for issue in "$fixture_issue" "$soft_issue" "$hard_issue"; do
            [[ -n "$issue" ]] || continue
            gh issue close "$issue" --repo "$repository" \
                --comment "Braid Slice 4 black-box fixture closed." >/dev/null 2>&1 || true
        done
    fi
    if [[ $exit_status -ne 0 ]]; then
        printf '%s: runtime log follows\n' "$script_name" >&2
        [[ -f "$runtime_log" ]] && tail -240 "$runtime_log" >&2 || true
        printf '%s: tunnel log follows\n' "$script_name" >&2
        [[ -f "$tunnel_log" ]] && tail -100 "$tunnel_log" >&2 || true
    fi
    if [[ -n "$temporary_root" && -d "$temporary_root" ]]; then
        rm -rf "$temporary_root"
    fi
    exit "$exit_status"
}
trap cleanup EXIT INT TERM

[[ -n "$binary" && -x "$binary" ]] || fail "set BRAID_BIN to the packaged braid binary"
[[ -n "$config_path" && "$config_path" = /* && -f "$config_path" ]] || \
    fail "set BRAID_CONFIG to an absolute acceptance configuration path"
[[ -n "${BRAID_WEBHOOK_SECRET:-}" ]] || \
    fail "BRAID_WEBHOOK_SECRET must match the temporary repository webhook secret"
for command in awk curl gh grep head jq mktemp ps sed shasum tr; do
    command -v "$command" >/dev/null 2>&1 || fail "required command is unavailable: $command"
done
gh auth status >/dev/null 2>&1 || fail "gh must expose the controlled Human/Agent identity"

repository="$($binary config check --config "$config_path" --json | jq -er '.repository')"
app_actor="$($binary github probe --config "$config_path" --repository "$repository" --json | jq -er '.actor_login')"
agent_actor="$(gh api user --jq '.login')"
wrangler="${BRAID_TEST_WRANGLER:-$(command -v wrangler || true)}"
[[ -n "$wrangler" && -x "$wrangler" ]] || fail "set BRAID_TEST_WRANGLER to Wrangler"
candidate_version="$($binary --version)"
candidate_sha256="$(shasum -a 256 "$binary" | sed 's/ .*//')"
note "candidate $candidate_version sha256=$candidate_sha256"

temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/braid-slice4.XXXXXX")"
runtime_log="$temporary_root/runtime.log"
tunnel_log="$temporary_root/tunnel.log"
readonly runtime_root="$temporary_root/runtime"
readonly test_config="$temporary_root/braid.toml"
mkdir -p "$runtime_root/state/backups"

# The controlled Human and Agent share one gh login in this PoC. Removing the
# optional stable actor declaration lets quote-block attribution identify Agent
# comments while plain Human edits remain external invalidations.
awk \
    -v root="$runtime_root" \
    -v database="$runtime_root/state/braid.sqlite3" \
    -v backups="$runtime_root/state/backups" \
    -v ingress="$ingress_address" \
    -v health="$health_address" '
    /^github_actor_node_id = / { next }
    /^\[runtime\]$/ { section="runtime"; print; next }
    /^\[server\]$/ { section="server"; print; next }
    /^\[/ && $0 != "[runtime]" && $0 != "[server]" { section="other" }
    section == "runtime" && /^root = / { printf "root = \"%s\"\n", root; next }
    section == "runtime" && /^database = / { printf "database = \"%s\"\n", database; next }
    section == "runtime" && /^backups = / { printf "backups = \"%s\"\n", backups; next }
    section == "runtime" && /^auto_migrate = / { print "auto_migrate = false"; next }
    section == "server" && /^ingress = / { printf "ingress = \"%s\"\n", ingress; next }
    section == "server" && /^health = / { printf "health = \"%s\"\n", health; next }
    { print }
' "$config_path" > "$test_config"

$binary migrate apply --config "$test_config" >/dev/null
$binary status --config "$test_config" --json | \
    jq -e '.database.schema_version == 8 and .database.supported_schema == 8' >/dev/null || \
    fail "candidate does not expose the expected current schema 8"

note "starting a free HTTP/2 Quick Tunnel"
TUNNEL_TRANSPORT_PROTOCOL=http2 "$wrangler" tunnel quick-start \
    "http://$ingress_address" >"$tunnel_log" 2>&1 &
tunnel_pid=$!
for _ in $(seq 1 90); do
    tunnel_url="$(grep -Eo 'https://[a-z0-9-]+\.trycloudflare\.com' "$tunnel_log" | tail -1 || true)"
    [[ -n "$tunnel_url" ]] && break
    kill -0 "$tunnel_pid" 2>/dev/null || fail "Quick Tunnel exited before publishing a URL"
    sleep 1
done
[[ -n "${tunnel_url:-}" ]] || fail "Quick Tunnel did not publish a URL"

BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$binary" serve \
    --config "$test_config" >"$runtime_log" 2>&1 &
runtime_pid=$!
for _ in $(seq 1 120); do
    if curl -fsS "$health_url" 2>/dev/null | \
        jq -e '.ready == true and .provider == "connected"' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during startup"
    sleep 1
done
curl -fsS "$health_url" | jq -e '.ready == true and .provider == "connected"' >/dev/null || \
    fail "Braid did not become ready"

note "verifying the public signed webhook boundary before creating GitHub fixtures"
public_probe_ready=0
for _ in $(seq 1 3); do
    if BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$binary" tunnel probe \
        --config "$test_config" --url "$tunnel_url/webhook" >/dev/null 2>&1; then
        public_probe_ready=1
        break
    fi
    sleep 5
done
[[ "$public_probe_ready" -eq 1 ]] || fail "public signed webhook probe failed"

readonly baseline_marker="BRAID_RESET_BASELINE_$(date -u +%s)"
readonly idle_marker="BRAID_IDLE_CONTEXT_$(date -u +%s)"
readonly active_marker="BRAID_ACTIVE_CONTEXT_$(date -u +%s)"
readonly restored_marker="BRAID_RESTORED_COMMENT_$(date -u +%s)"
readonly deleted_marker="BRAID_DELETED_COMMENT_$(date -u +%s)"
readonly preclose_marker="BRAID_PRECLOSE_REVIEW_$(date -u +%s)"
readonly reopened_marker="BRAID_REOPENED_CONTEXT_$(date -u +%s)"
readonly resumed_marker="BRAID_PROVIDER_RESUMED_$(date -u +%s)"
fixture_issue="$(gh api --method POST "repos/$repository/issues" \
    -f title="Braid Slice 4: Context lifecycle $(date -u +%Y%m%dT%H%M%SZ)" \
    -f body="Baseline design: $baseline_marker" --jq '.number')"
restored_comment_json="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="Lifecycle fixture $restored_marker. Only when Braid reports that this comment was unminimized, publish one concise attributed comment containing $restored_marker.")"
restored_comment_node_id="$(jq -er '.node_id' <<<"$restored_comment_json")"
deleted_comment_json="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="Lifecycle deletion fixture: $deleted_marker")"
deleted_comment_id="$(jq -er '.id' <<<"$deleted_comment_json")"

# Register transport only after the baseline lifecycle comments exist. They
# enter the initial canonical Context without manufacturing pre-activation
# Wake batches.
repository_hook_id="$(gh api --method POST "repos/$repository/hooks" \
    -f name=web \
    -F active=true \
    -f 'events[]=issues' \
    -f 'events[]=issue_comment' \
    -f "config[url]=$tunnel_url/webhook" \
    -f 'config[content_type]=json' \
    -f "config[secret]=$BRAID_WEBHOOK_SECRET" \
    --jq '.id')"

has_reaction() {
    local comment_id=$1
    local content=$2
    gh api "repos/$repository/issues/comments/$comment_id/reactions" \
        --jq ".[] | select(.content == \"$content\" and .user.login == \"$app_actor\") | .id" | \
        grep -q .
}

agent_marker_count() {
    local marker=$1
    local issue=${2:-$fixture_issue}
    gh api "repos/$repository/issues/$issue/comments" | \
        jq --arg actor "$agent_actor" --arg marker "$marker" \
            '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")) and (.body | contains($marker)))] | length'
}

operational_status_count() {
    local issue=$1
    local phrase=$2
    gh api "repos/$repository/issues/$issue/comments" | \
        jq --arg actor "$app_actor" --arg phrase "$phrase" \
            '[.[] | select(.user.login == $actor and (.body | contains($phrase)))] | length'
}

write_pressure_config() {
    local destination=$1
    local root=$2
    local soft_ratio=$3
    local hard_bytes=$4
    mkdir -p "$root/state/backups"
    awk \
        -v root="$root" \
        -v database="$root/state/braid.sqlite3" \
        -v backups="$root/state/backups" \
        -v soft_ratio="$soft_ratio" \
        -v hard_bytes="$hard_bytes" '
        /^\[runtime\]$/ { section="runtime"; print; next }
        /^\[/ && $0 != "[runtime]" { section="other" }
        section == "runtime" && /^root = / { printf "root = \"%s\"\n", root; next }
        section == "runtime" && /^database = / { printf "database = \"%s\"\n", database; next }
        section == "runtime" && /^backups = / { printf "backups = \"%s\"\n", backups; next }
        /^github_context_soft_ratio = / { printf "github_context_soft_ratio = %s\n", soft_ratio; next }
        /^github_context_hard_bytes = / { printf "github_context_hard_bytes = %s\n", hard_bytes; next }
        { print }
    ' "$test_config" > "$destination"
}

start_candidate_runtime() {
    local config=$1
    BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$binary" serve \
        --config "$config" >>"$runtime_log" 2>&1 &
    runtime_pid=$!
    for _ in $(seq 1 120); do
        if curl -fsS "$health_url" 2>/dev/null | \
            jq -e '.ready == true and .provider == "connected"' >/dev/null; then
            return 0
        fi
        kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited during startup"
        sleep 1
    done
    fail "Braid did not become ready"
}

note "activating one baseline Issue Agent session"
activation_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="@braid Read the current Issue design and publish one concise attributed comment containing $baseline_marker." --jq '.id')"
for _ in $(seq 1 180); do
    has_reaction "$activation_comment" +1 && \
        [[ "$(agent_marker_count "$baseline_marker")" -eq 1 ]] && break
    sleep 2
done
has_reaction "$activation_comment" +1 || fail "baseline activation did not complete"
[[ "$(agent_marker_count "$baseline_marker")" -eq 1 ]] || fail "baseline Agent marker is absent"

for _ in $(seq 1 90); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" '
        any(.transport.agent_groups[];
          .work_item_kind == "issue" and .work_item_number == $number and
          .session_lifecycle == "idle" and .active_turn_id == null)
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
jq -e --argjson number "$fixture_issue" '
    any(.transport.agent_groups[];
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle" and .active_turn_id == null)
' >/dev/null <<<"$status_payload" || fail "baseline turn did not converge to idle"
baseline_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][0].provider_session_id
' <<<"$status_payload")"
baseline_agent_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$agent_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"

note "editing idle Issue Context: replace session without starting a turn"
gh issue edit "$fixture_issue" --repo "$repository" --body "Idle replacement design: $idle_marker" >/dev/null
for _ in $(seq 1 120); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" '
        any(.transport.context_resets[];
          .work_item_number == $number and .lifecycle == "applied" and
          .continuation == false and .context_revision_before != .context_revision_after)
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
jq -e --argjson number "$fixture_issue" '
    any(.transport.context_resets[];
      .work_item_number == $number and .lifecycle == "applied" and
      .continuation == false and .context_revision_before != .context_revision_after)
' >/dev/null <<<"$status_payload" || fail "idle Hard Invalidation did not apply"
idle_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][-1].provider_session_id
' <<<"$status_payload")"
[[ "$idle_session" != "$baseline_session" ]] || fail "idle invalidation reused the stale provider session"
sleep 5
current_agent_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$agent_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"
[[ "$current_agent_comments" -eq "$baseline_agent_comments" ]] || \
    fail "idle invalidation fabricated a turn"

note "editing Issue Context during a turn: fence, interrupt, replace, continue"
active_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="@braid Please review whether the current Issue description is internally coherent and publish one concise attributed assessment that names its current BRAID_* design marker." --jq '.id')"
for _ in $(seq 1 90); do
    has_reaction "$active_comment" rocket && break
    sleep 1
done
has_reaction "$active_comment" rocket || fail "active invalidation fixture was not accepted"
gh issue edit "$fixture_issue" --repo "$repository" --body "Active replacement design: $active_marker" >/dev/null

for _ in $(seq 1 240); do
    status_payload="$($binary status --config "$test_config" --json)"
    active_reset_count="$(jq --argjson number "$fixture_issue" \
        '[.transport.context_resets[] | select(.work_item_number == $number and .lifecycle == "applied" and .continuation == true)] | length' \
        <<<"$status_payload")"
    if [[ "$active_reset_count" -eq 1 ]] && [[ "$(agent_marker_count "$active_marker")" -eq 1 ]]; then
        break
    fi
    sleep 2
done
[[ "${active_reset_count:-0}" -eq 1 ]] || fail "active Hard Invalidation did not apply exactly once"
[[ "$(agent_marker_count "$active_marker")" -eq 1 ]] || \
    fail "continuation did not use the replacement Context"
for _ in $(seq 1 30); do
    has_reaction "$active_comment" rocket || break
    sleep 1
done
has_reaction "$active_comment" rocket && fail "superseded turn retained rocket"
has_reaction "$active_comment" +1 && fail "superseded turn was reported successful"
has_reaction "$active_comment" confused && fail "superseded turn was reported failed"

for _ in $(seq 1 90); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" '
        any(.transport.agent_groups[];
          .work_item_kind == "issue" and .work_item_number == $number and
          .session_lifecycle == "idle" and .active_turn_id == null)
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
jq -e --argjson number "$fixture_issue" '
    any(.transport.agent_groups[];
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle" and .active_turn_id == null)
' >/dev/null <<<"$status_payload" || fail "replacement continuation did not converge to idle"

active_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][-1].provider_session_id
' <<<"$status_payload")"
[[ "$active_session" != "$idle_session" ]] || fail "active invalidation reused the stale session"
distinct_sessions="$(jq --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(.work_item_number == $number) | .provider_session_id] |
    unique | length
' <<<"$status_payload")"
[[ "$distinct_sessions" -eq 3 ]] || fail "expected exactly three physical sessions, got $distinct_sessions"

note "minimizing a visible comment: reconcile, replace idle Context, start no turn"
comments_before_minimize="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$agent_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"
gh api graphql \
    -f query='mutation($id:ID!){minimizeComment(input:{subjectId:$id,classifier:OUTDATED}){minimizedComment{isMinimized minimizedReason}}}' \
    -f id="$restored_comment_node_id" | jq -e '.data.minimizeComment.minimizedComment.isMinimized == true' >/dev/null
for _ in $(seq 1 180); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" --arg old "$active_session" '
        any(.transport.context_resets[];
          .work_item_number == $number and .old_provider_session_id == $old and
          .lifecycle == "applied" and .continuation == false and
          .context_revision_before != .context_revision_after)
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
jq -e --argjson number "$fixture_issue" --arg old "$active_session" '
    any(.transport.context_resets[];
      .work_item_number == $number and .old_provider_session_id == $old and
      .lifecycle == "applied" and .continuation == false and
      .context_revision_before != .context_revision_after)
' >/dev/null <<<"$status_payload" || fail "minimized comment did not replace idle Context"
minimized_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][-1].provider_session_id
' <<<"$status_payload")"
[[ "$minimized_session" != "$active_session" ]] || fail "minimized comment reused stale Context"
minimized_context="$($binary context issue "$repository#$fixture_issue" --config "$test_config")"
grep -q 'State: minimized (outdated)' <<<"$minimized_context" || \
    fail "minimized comment metadata is absent from current Context"
grep -q "$restored_marker" <<<"$minimized_context" && \
    fail "minimized comment body remained in current Context"
sleep 5
comments_after_minimize="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$agent_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"
[[ "$comments_after_minimize" -eq "$comments_before_minimize" ]] || \
    fail "minimize Hard Invalidation fabricated a turn"

note "unminimizing the comment: restore body and release one ordinary Wake"
gh api graphql \
    -f query='mutation($id:ID!){unminimizeComment(input:{subjectId:$id}){unminimizedComment{isMinimized}}}' \
    -f id="$restored_comment_node_id" | jq -e '.data.unminimizeComment.unminimizedComment.isMinimized == false' >/dev/null
for _ in $(seq 1 240); do
    status_payload="$($binary status --config "$test_config" --json)"
    if [[ "$(agent_marker_count "$restored_marker")" -eq 1 ]] && \
        jq -e --argjson number "$fixture_issue" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .session_lifecycle == "idle" and
            .active_turn_id == null)
        ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
[[ "$(agent_marker_count "$restored_marker")" -eq 1 ]] || \
    fail "unminimize Wake did not expose the restored comment body to the Agent"
restored_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][-1].provider_session_id
' <<<"$status_payload")"
[[ "$restored_session" == "$minimized_session" ]] || \
    fail "unminimize Wake unexpectedly replaced the valid provider session"
restored_context="$($binary context issue "$repository#$fixture_issue" --config "$test_config")"
grep -q "$restored_marker" <<<"$restored_context" || \
    fail "unminimized body is absent from current Context"

note "deleting another visible comment: retain tombstone, replace idle Context"
comments_before_delete="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$agent_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"
gh api --method DELETE "repos/$repository/issues/comments/$deleted_comment_id" >/dev/null
for _ in $(seq 1 150); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" --arg old "$restored_session" '
        any(.transport.context_resets[];
          .work_item_number == $number and .old_provider_session_id == $old and
          .lifecycle == "applied" and .continuation == false and
          .context_revision_before != .context_revision_after)
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
jq -e --argjson number "$fixture_issue" --arg old "$restored_session" '
    any(.transport.context_resets[];
      .work_item_number == $number and .old_provider_session_id == $old and
      .lifecycle == "applied" and .continuation == false and
      .context_revision_before != .context_revision_after)
' >/dev/null <<<"$status_payload" || fail "deleted comment did not replace idle Context"
deleted_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][-1].provider_session_id
' <<<"$status_payload")"
[[ "$deleted_session" != "$restored_session" ]] || fail "deleted comment reused stale Context"
deleted_context="$($binary context issue "$repository#$fixture_issue" --config "$test_config")"
grep -q 'State: deleted' <<<"$deleted_context" || \
    fail "deleted comment tombstone is absent from current Context"
grep -q "$deleted_marker" <<<"$deleted_context" && \
    fail "deleted comment body remained in current Context"
sleep 5
comments_after_delete="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$agent_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"
[[ "$comments_after_delete" -eq "$comments_before_delete" ]] || \
    fail "delete Hard Invalidation fabricated a turn"

distinct_sessions="$(jq --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(.work_item_number == $number) | .provider_session_id] |
    unique | length
' <<<"$status_payload")"
[[ "$distinct_sessions" -eq 5 ]] || fail "expected five physical sessions, got $distinct_sessions"

note "closing during an active turn: finish current turn, run one finalization, then sleep"
preclose_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="@braid Conduct a careful final design audit before closure and publish one concise attributed comment containing $preclose_marker." --jq '.id')"
for _ in $(seq 1 90); do
    status_payload="$($binary status --config "$test_config" --json)"
    if has_reaction "$preclose_comment" rocket && \
        jq -e --argjson number "$fixture_issue" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .session_lifecycle == "running" and
            .active_turn_id != null)
        ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
has_reaction "$preclose_comment" rocket || fail "pre-close turn was not accepted"
jq -e --argjson number "$fixture_issue" '
    any(.transport.agent_groups[];
      .work_item_number == $number and .session_lifecycle == "running" and
      .active_turn_id != null)
' >/dev/null <<<"$status_payload" || fail "Issue was not active before close"
gh issue close "$fixture_issue" --repo "$repository" >/dev/null

for _ in $(seq 1 240); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" '
        any(.transport.agent_groups[];
          .work_item_number == $number and .assignment_lifecycle == "sleeping" and
          .session_lifecycle == "sleeping" and .active_turn_id == null and
          .finalization_turns == 1 and .last_finalization_lifecycle == "completed")
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 2
done
jq -e --argjson number "$fixture_issue" '
    any(.transport.agent_groups[];
      .work_item_number == $number and .assignment_lifecycle == "sleeping" and
      .session_lifecycle == "sleeping" and .active_turn_id == null and
      .finalization_turns == 1 and .last_finalization_lifecycle == "completed")
' >/dev/null <<<"$status_payload" || fail "close did not converge through one finalization to sleeping"
has_reaction "$preclose_comment" +1 || fail "close interrupted the already-running turn"
has_reaction "$preclose_comment" confused && fail "close reported the already-running turn failed"
[[ "$(agent_marker_count "$preclose_marker")" -eq 1 ]] || \
    fail "pre-close turn did not publish its expected assessment"

note "adding work while closed: retain it in GitHub Context without granting another turn"
closed_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="When this Issue reopens, read the complete current Context and publish one concise attributed comment containing $reopened_marker." --jq '.id')"
for _ in $(seq 1 90); do
    has_reaction "$closed_comment" eyes && break
    sleep 1
done
has_reaction "$closed_comment" eyes || fail "closed-Issue comment was not durably acknowledged"
sleep 35
status_payload="$($binary status --config "$test_config" --json)"
jq -e --argjson number "$fixture_issue" '
    any(.transport.agent_groups[];
      .work_item_number == $number and .assignment_lifecycle == "sleeping" and
      .finalization_turns == 1)
' >/dev/null <<<"$status_payload" || fail "closed-Issue activity woke the sleeping Agent Group"
[[ "$(agent_marker_count "$reopened_marker")" -eq 0 ]] || \
    fail "closed-Issue activity started a second turn"

note "reopening: rebuild current Context in a fresh session and release one debounced Wake"
gh issue reopen "$fixture_issue" --repo "$repository" >/dev/null
for _ in $(seq 1 300); do
    status_payload="$($binary status --config "$test_config" --json)"
    if [[ "$(agent_marker_count "$reopened_marker")" -eq 1 ]] && \
        jq -e --argjson number "$fixture_issue" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .assignment_lifecycle == "active" and
            .session_lifecycle == "idle" and .active_turn_id == null and
            .finalization_turns == 1)
        ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
[[ "$(agent_marker_count "$reopened_marker")" -eq 1 ]] || \
    fail "reopen Wake did not use the current GitHub Context"
reopened_session="$(jq -er --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "idle")][-1].provider_session_id
' <<<"$status_payload")"
[[ "$reopened_session" != "$deleted_session" ]] || fail "reopen reused the sleeping provider session"
distinct_sessions="$(jq --argjson number "$fixture_issue" '
    [.transport.agent_groups[] | select(.work_item_number == $number) | .provider_session_id] |
    unique | length
' <<<"$status_payload")"
[[ "$distinct_sessions" -eq 6 ]] || fail "expected six physical sessions after reopen, got $distinct_sessions"

note "terminating the idle app-server: reconnect and resume the same provider thread"
provider_child_pid=""
for _ in $(seq 1 30); do
    provider_child_pid="$(ps -axo pid=,ppid=,command= | awk -v parent="$runtime_pid" '
        $2 == parent && index($0, "app-server") { print $1; exit }
    ')"
    [[ -n "$provider_child_pid" ]] && break
    sleep 1
done
[[ -n "$provider_child_pid" ]] || fail "could not identify the app-server child process"
kill -TERM "$provider_child_pid"
for _ in $(seq 1 120); do
    status_payload="$($binary status --config "$test_config" --json)"
    if curl -fsS "$health_url" 2>/dev/null | jq -e '.provider == "connected"' >/dev/null && \
        jq -e --argjson number "$fixture_issue" --arg session "$reopened_session" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .provider_session_id == $session and
            .session_lifecycle == "idle" and .provider_resume_count >= 1 and
            .last_provider_resume != null)
        ' >/dev/null <<<"$status_payload"; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "Braid exited while reconnecting app-server"
    sleep 1
done
jq -e --argjson number "$fixture_issue" --arg session "$reopened_session" '
    any(.transport.agent_groups[];
      .work_item_number == $number and .provider_session_id == $session and
      .session_lifecycle == "idle" and .provider_resume_count >= 1 and
      .last_provider_resume != null)
' >/dev/null <<<"$status_payload" || fail "provider thread did not resume in place"

resume_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="After the debounce window, publish one concise attributed comment containing $resumed_marker." --jq '.id')"
for _ in $(seq 1 240); do
    status_payload="$($binary status --config "$test_config" --json)"
    if [[ "$(agent_marker_count "$resumed_marker")" -eq 1 ]] && \
        jq -e --argjson number "$fixture_issue" --arg session "$reopened_session" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .provider_session_id == $session and
            .session_lifecycle == "idle" and .active_turn_id == null)
        ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
has_reaction "$resume_comment" eyes || fail "post-resume Wake was not durably acknowledged"
has_reaction "$resume_comment" rocket && fail "ordinary post-resume Wake received request-style reaction"
[[ "$(agent_marker_count "$resumed_marker")" -eq 1 ]] || \
    fail "resumed provider thread did not handle the next debounced Wake"
jq -e --argjson number "$fixture_issue" --arg session "$reopened_session" '
    any(.transport.agent_groups[];
      .work_item_number == $number and .provider_session_id == $session and
      .session_lifecycle == "idle" and .active_turn_id == null)
' >/dev/null <<<"$status_payload" || fail "post-resume turn changed the physical provider session"

app_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$app_actor" '[.[] | select(.user.login == $actor)] | length')"
[[ "$app_comments" -eq 0 ]] || fail "Braid published turn activity during Context replacement"

stop_process "$runtime_pid"
runtime_pid=""

note "activating a soft-pressure Context: publish status but supply the complete Context"
readonly soft_root="$temporary_root/soft-runtime"
readonly soft_config="$temporary_root/soft-braid.toml"
write_pressure_config "$soft_config" "$soft_root" 0.10 20000
$binary migrate apply --config "$soft_config" >/dev/null
start_candidate_runtime "$soft_config"
soft_payload="$(head -c 4096 /dev/zero | tr '\0' S)"
soft_marker="BRAID_SOFT_CONTEXT_$(date -u +%s)"
soft_issue="$(gh api --method POST "repos/$repository/issues" \
    -f title="Braid Slice 4: soft Context pressure" \
    -f body="Soft-pressure design fixture. $soft_payload" --jq '.number')"
soft_activation="$(gh api --method POST "repos/$repository/issues/$soft_issue/comments" \
    -f body="@braid Publish one concise attributed comment containing $soft_marker." --jq '.id')"
for _ in $(seq 1 300); do
    soft_status="$($binary status --config "$soft_config" --json)"
    if [[ "$(agent_marker_count "$soft_marker" "$soft_issue")" -eq 1 ]] && \
        [[ "$(operational_status_count "$soft_issue" "near the Profile limit")" -eq 1 ]] && \
        jq -e --argjson number "$soft_issue" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .assignment_lifecycle == "active" and
            .session_lifecycle == "idle" and .active_turn_id == null and
            .context_pressure == "soft" and .context_bytes > 0)
        ' >/dev/null <<<"$soft_status"; then
        break
    fi
    sleep 1
done
has_reaction "$soft_activation" +1 || fail "soft-pressure turn did not complete"
[[ "$(agent_marker_count "$soft_marker" "$soft_issue")" -eq 1 ]] || \
    fail "soft-pressure Agent did not receive the complete Context"
[[ "$(operational_status_count "$soft_issue" "near the Profile limit")" -eq 1 ]] || \
    fail "soft pressure did not converge to one Operational Status Comment"
soft_context_bytes="$(jq -er --argjson number "$soft_issue" '
    [.transport.agent_groups[] | select(
      .work_item_number == $number and .context_pressure == "soft")][0].context_bytes
' <<<"$soft_status")"
[[ "$soft_context_bytes" -lt 20000 ]] || fail "soft fixture crossed the hard limit"

stop_process "$runtime_pid"
runtime_pid=""

note "activating a hard-pressure Context: block without truncation or a provider session"
readonly hard_root="$temporary_root/hard-runtime"
readonly hard_config="$temporary_root/hard-braid.toml"
write_pressure_config "$hard_config" "$hard_root" 0.80 512
$binary migrate apply --config "$hard_config" >/dev/null
start_candidate_runtime "$hard_config"
hard_payload="$(head -c 2048 /dev/zero | tr '\0' H)"
hard_marker="BRAID_HARD_CONTEXT_$(date -u +%s)"
hard_issue="$(gh api --method POST "repos/$repository/issues" \
    -f title="Braid Slice 4: hard Context pressure" \
    -f body="Hard-pressure design fixture. $hard_payload" --jq '.number')"
hard_activation="$(gh api --method POST "repos/$repository/issues/$hard_issue/comments" \
    -f body="@braid Do not truncate this Context. If accepted, publish $hard_marker." --jq '.id')"
for _ in $(seq 1 180); do
    hard_status="$($binary status --config "$hard_config" --json)"
    if has_reaction "$hard_activation" eyes && \
        [[ "$(operational_status_count "$hard_issue" "Context is too large")" -eq 1 ]] && \
        jq -e --argjson number "$hard_issue" '
          any(.transport.agent_groups[];
            .work_item_number == $number and .assignment_lifecycle == "blocked" and
            .provider_session_id == null and .session_lifecycle == null and
            .context_pressure == "hard" and .context_bytes > 512)
        ' >/dev/null <<<"$hard_status"; then
        break
    fi
    sleep 1
done
has_reaction "$hard_activation" eyes || fail "hard-pressure event was not durably acknowledged"
has_reaction "$hard_activation" rocket && fail "hard-pressure event started a provider turn"
has_reaction "$hard_activation" +1 && fail "hard-pressure event was reported successful"
has_reaction "$hard_activation" confused && fail "hard-pressure event was reported as Agent failure"
[[ "$(agent_marker_count "$hard_marker" "$hard_issue")" -eq 0 ]] || \
    fail "hard-pressure Context reached an Agent"
[[ "$(operational_status_count "$hard_issue" "Context is too large")" -eq 1 ]] || \
    fail "hard pressure did not converge to one Operational Status Comment"
jq -e --argjson number "$hard_issue" '
    any(.transport.agent_groups[];
      .work_item_number == $number and .assignment_lifecycle == "blocked" and
      .provider_session_id == null and .context_pressure == "hard" and
      .context_bytes > 512)
' >/dev/null <<<"$hard_status" || fail "hard pressure did not remain observable and blocked"

stop_process "$runtime_pid"
runtime_pid=""
stop_process "$tunnel_pid"
tunnel_pid=""
gh api --method DELETE "repos/$repository/hooks/$repository_hook_id" >/dev/null
repository_hook_id=""

jq -n \
    --arg repository "$repository" \
    --arg candidate "$candidate_version" \
    --arg candidate_sha256 "$candidate_sha256" \
    --arg baseline_session "$baseline_session" \
    --arg idle_session "$idle_session" \
    --arg active_session "$active_session" \
    --arg minimized_session "$minimized_session" \
    --arg deleted_session "$deleted_session" \
    --arg reopened_session "$reopened_session" \
    --argjson issue "$fixture_issue" \
    --argjson soft_issue "$soft_issue" \
    --argjson hard_issue "$hard_issue" \
    --argjson soft_context_bytes "$soft_context_bytes" \
    '{
      verdict:"PASS",
      boundary:"real GitHub -> HTTP/2 Quick Tunnel -> packaged Braid -> real Codex app-server",
      repository:$repository,
      candidate:$candidate,
      candidate_sha256:$candidate_sha256,
      fixture_issue:$issue,
      pressure_fixtures:{soft_issue:$soft_issue,hard_issue:$hard_issue,soft_context_bytes:$soft_context_bytes},
      sessions:{
        baseline:$baseline_session,
        idle_replacement:$idle_session,
        active_replacement:$active_session,
        minimized_replacement:$minimized_session,
        deleted_replacement:$deleted_session,
        reopened:$reopened_session
      },
      journeys:[
        "idle-hard-invalidation",
        "active-hard-invalidation",
        "stale-turn-reaction-fence",
        "continuation-current-context",
        "minimize-reconciliation-reset",
        "unminimize-wake",
        "delete-tombstone-reset",
        "active-close-finalization-sleep",
        "closed-activity-no-wake",
        "reopen-fresh-context-debounced-wake",
        "provider-reconnect-same-thread",
        "post-resume-debounced-wake",
        "soft-pressure-complete-context",
        "hard-pressure-block-no-truncation"
      ]
    }'
