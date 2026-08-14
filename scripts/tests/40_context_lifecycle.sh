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
    if [[ -n "$fixture_issue" && "$keep_fixture" != "1" ]]; then
        gh issue close "$fixture_issue" --repo "$repository" \
            --comment "Braid Slice 4 black-box fixture closed." >/dev/null 2>&1 || true
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
for command in curl gh grep jq mktemp sed shasum; do
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
    jq -e '.database.schema_version == 6 and .database.supported_schema == 6' >/dev/null || \
    fail "candidate does not expose the expected Slice 4 schema 6"

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
    gh api "repos/$repository/issues/$fixture_issue/comments" | \
        jq --arg actor "$agent_actor" --arg marker "$marker" \
            '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")) and (.body | contains($marker)))] | length'
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

app_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$app_actor" '[.[] | select(.user.login == $actor)] | length')"
[[ "$app_comments" -eq 0 ]] || fail "Braid published turn activity during Context replacement"

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
    --argjson issue "$fixture_issue" \
    '{
      verdict:"PASS",
      boundary:"real GitHub -> HTTP/2 Quick Tunnel -> packaged Braid -> real Codex app-server",
      repository:$repository,
      candidate:$candidate,
      candidate_sha256:$candidate_sha256,
      fixture_issue:$issue,
      sessions:{
        baseline:$baseline_session,
        idle_replacement:$idle_session,
        active_replacement:$active_session,
        minimized_replacement:$minimized_session,
        deleted_replacement:$deleted_session
      },
      journeys:[
        "idle-hard-invalidation",
        "active-hard-invalidation",
        "stale-turn-reaction-fence",
        "continuation-current-context",
        "minimize-reconciliation-reset",
        "unminimize-wake",
        "delete-tombstone-reset"
      ]
    }'
