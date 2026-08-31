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
failure_issue=""
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

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command is unavailable: $1"
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
    for _ in $(seq 1 5); do
        kill -0 "$pid" 2>/dev/null || { wait "$pid" 2>/dev/null || true; return 0; }
        sleep 1
    done
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
            --comment "Braid Slice 3 black-box fixture closed." >/dev/null 2>&1 || true
    fi
    if [[ -n "$failure_issue" && "$keep_fixture" != "1" ]]; then
        gh issue close "$failure_issue" --repo "$repository" \
            --comment "Braid Slice 3 failure fixture closed." >/dev/null 2>&1 || true
    fi
    if [[ -n "$temporary_root" && -d "$temporary_root" ]]; then
        rm -rf "$temporary_root"
    fi
    if [[ $exit_status -ne 0 ]]; then
        printf '%s: runtime log follows\n' "$script_name" >&2
        [[ -f "$runtime_log" ]] && tail -200 "$runtime_log" >&2 || true
        printf '%s: tunnel log follows\n' "$script_name" >&2
        [[ -f "$tunnel_log" ]] && tail -100 "$tunnel_log" >&2 || true
    fi
    exit "$exit_status"
}
trap cleanup EXIT INT TERM

[[ -n "$binary" && -x "$binary" ]] || fail "set BRAID_BIN to the packaged braid binary"
[[ -n "$config_path" && "$config_path" = /* && -f "$config_path" ]] || \
    fail "set BRAID_CONFIG to an absolute acceptance configuration path"
[[ -n "${BRAID_WEBHOOK_SECRET:-}" ]] || \
    fail "BRAID_WEBHOOK_SECRET must match the dedicated acceptance webhook secret"
for command in curl gh grep jq mktemp pgrep sed shasum; do
    require_command "$command"
done
gh auth status >/dev/null 2>&1 || fail "gh must expose the configured Coding Agent identity"

repository="$($binary config check --config "$config_path" --json | jq -er '.repository')"
app_actor="$($binary github probe --config "$config_path" --repository "$repository" --json | jq -er '.actor_login')"
agent_actor="$(gh api user --jq '.login')"
wrangler="${BRAID_TEST_WRANGLER:-$(command -v wrangler || true)}"
public_url="${BRAID_TEST_PUBLIC_WEBHOOK_URL:-}"
public_url="${public_url%/webhook}"
if [[ -z "$public_url" ]]; then
    [[ -n "$wrangler" && -x "$wrangler" ]] || fail "set BRAID_TEST_WRANGLER to Wrangler"
fi
candidate_version="$($binary --version)"
candidate_sha256="$(shasum -a 256 "$binary" | sed 's/ .*//')"
note "candidate $candidate_version sha256=$candidate_sha256"

temporary_root="$(mktemp -d "${TMPDIR:-/tmp}/braid-slice3.XXXXXX")"
runtime_log="$temporary_root/runtime.log"
tunnel_log="$temporary_root/tunnel.log"
readonly runtime_root="$temporary_root/runtime"
readonly test_config="$temporary_root/braid.toml"
mkdir -p "$runtime_root/state/backups"

awk \
    -v root="$runtime_root" \
    -v database="$runtime_root/state/braid.sqlite3" \
    -v backups="$runtime_root/state/backups" \
    -v ingress="$ingress_address" \
    -v health="$health_address" '
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
    jq -e '.database.schema_version == 2 and .database.supported_schema == 2' >/dev/null || \
    fail "candidate does not expose the expected current schema 1"

public_url="${BRAID_TEST_PUBLIC_WEBHOOK_URL:-}"
public_url="${public_url%/webhook}"
if [[ -n "$public_url" ]]; then
    note "using external public webhook URL"
    tunnel_pid=""
else
    note "starting a free HTTP/2 Quick Tunnel"
    TUNNEL_TRANSPORT_PROTOCOL=http2 "$wrangler" tunnel quick-start \
        "http://$ingress_address" --log-level info >"$tunnel_log" 2>&1 &
    tunnel_pid=$!
    public_url=""
    for _ in $(seq 1 60); do
        public_url="$(grep -Eo 'https://[a-z0-9-]+\.(trycloudflare\.com|loca\.lt)' "$tunnel_log" | head -1 || true)"
        grep -q 'Registered tunnel connection' "$tunnel_log" 2>/dev/null && \
            [[ -n "$public_url" ]] && break
        kill -0 "$tunnel_pid" 2>/dev/null || fail "Quick Tunnel exited during startup"
        sleep 1
    done
    [[ -n "$public_url" ]] || fail "Quick Tunnel did not publish a URL"
fi

note "starting packaged Braid with the real Codex app-server"
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
    fail "Braid did not become provider-ready"

repository_hook_id="$(jq -nc \
    --arg url "$public_url/webhook" \
    '{name:"web",active:true,events:["issues","issue_comment"],config:{url:$url,content_type:"json",insecure_ssl:"0",secret:env.BRAID_WEBHOOK_SECRET}}' | \
    gh api --method POST "repos/$repository/hooks" --input - --jq '.id')"
[[ -n "$repository_hook_id" ]] || fail "GitHub did not create the temporary repository webhook"

note "activating a dormant Issue through the trusted-mention fallback"
fixture_issue="$(gh api --method POST "repos/$repository/issues" \
    -f title="Braid Slice 3: Issue Agent $(date -u +%Y%m%dT%H%M%SZ)" \
    -f body='Disposable real-object fixture for the Braid Issue Agent gate.' --jq '.number')"
mention_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body='@braid Acceptance request: read this Issue, wait at least 8 seconds with a shell command, then publish exactly one concise comment saying the real Issue Agent turn completed. Begin with the required Braid Agent attribution quote.' --jq '.id')"

has_reaction() {
    local comment_id=$1
    local content=$2
    gh api "repos/$repository/issues/comments/$comment_id/reactions" \
        --jq ".[] | select(.content == \"$content\" and .user.login == \"$app_actor\") | .id" | \
        grep -q .
}

agent_marker_count() {
    local issue=$1
    local marker=$2
    gh api "repos/$repository/issues/$issue/comments" | jq --arg actor "$app_actor" --arg marker "$marker" \
        '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")) and (.body | contains($marker)))] | length'
}

for _ in $(seq 1 90); do
    has_reaction "$mention_comment" eyes && has_reaction "$mention_comment" rocket && break
    sleep 1
done
has_reaction "$mention_comment" eyes || fail "Braid did not acknowledge the mention with eyes"
has_reaction "$mention_comment" rocket || fail "Braid did not expose the accepted active turn with rocket"

note "steering the same active turn with an edited trusted mention"
gh api --method PATCH "repos/$repository/issues/comments/$mention_comment" \
    -f body='@braid Acceptance update: keep the same concise completion comment; this edit must steer the active turn rather than start a parallel turn.' >/dev/null

for _ in $(seq 1 180); do
    agent_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" 2>/dev/null | \
        jq --arg actor "$app_actor" \
            '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length' || true)"
    if has_reaction "$mention_comment" +1 && [[ "${agent_comments:-0}" -eq 1 ]]; then
        break
    fi
    sleep 2
done
has_reaction "$mention_comment" +1 || fail "Braid did not expose normal terminal with +1"
[[ "${agent_comments:-0}" -eq 1 ]] || fail "Agent did not publish exactly one attributed comment"

for _ in $(seq 1 30); do
    has_reaction "$mention_comment" rocket || break
    sleep 1
done
has_reaction "$mention_comment" rocket && fail "terminal reaction state retained stale rocket"

sleep 3
status_payload="$($binary status --config "$test_config" --json)"
jq -e --argjson number "$fixture_issue" '
    [.transport.agent_groups[] |
      select(.work_item_kind == "issue" and .work_item_number == $number and
             .assignment_lifecycle == "active" and .session_lifecycle == "idle" and
             .active_turn_id == null)] | length == 1
' >/dev/null <<<"$status_payload" || fail "Issue Agent did not converge to one idle session"
app_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
    jq --arg actor "$app_actor" '[.[] | select(.user.login == $actor)] | length')"
[[ "$app_comments" -eq 1 ]] || fail "Braid did not publish exactly one App-authored comment"

note "proving one ordinary comment waits for the complete quiet window"
ordinary_marker="BRAID_ORDINARY_COMPLETE_$(date -u +%s)"
ordinary_started="$(date +%s)"
ordinary_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="Ordinary event: after Braid releases this batch, publish one concise attributed comment containing $ordinary_marker. Do not mention the configured handle." --jq '.id')"
for _ in $(seq 1 30); do
    has_reaction "$ordinary_comment" eyes && break
    sleep 1
done
has_reaction "$ordinary_comment" eyes || fail "ordinary comment did not receive eyes"
while [[ "$(($(date +%s) - ordinary_started))" -lt 30 ]]; do
    [[ "$(agent_marker_count "$fixture_issue" "$ordinary_marker")" -eq 0 ]] || \
        fail "ordinary turn produced Agent output before the 30-second quiet window"
    sleep 2
done
for _ in $(seq 1 60); do
    [[ "$(agent_marker_count "$fixture_issue" "$ordinary_marker")" -eq 1 ]] && break
    sleep 2
done
[[ "$(agent_marker_count "$fixture_issue" "$ordinary_marker")" -eq 1 ]] || \
    fail "ordinary debounced turn did not publish exactly one Agent marker"
for reaction in rocket +1 confused; do
    has_reaction "$ordinary_comment" "$reaction" && \
        fail "ordinary turn exposed request-like $reaction reaction"
done

note "proving eight ordinary events release one count-threshold turn"
count_marker="BRAID_COUNT_COMPLETE_$(date -u +%s)"
count_comments=()
for index in $(seq 1 7); do
    count_comments+=("$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
        -f body="Count-threshold event $index of 8. Do not publish yet and do not mention the configured handle." --jq '.id')")
    sleep 1
done
count_comments+=("$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body="Count-threshold event 8 of 8: run a shell sleep for 8 seconds, then publish one concise attributed comment containing $count_marker. Do not mention the configured handle." --jq '.id')")
count_durable=0
for _ in $(seq 1 90); do
    count_durable=1
    for comment_id in "${count_comments[@]}"; do
        if ! has_reaction "$comment_id" eyes; then
            count_durable=0
            break
        fi
    done
    [[ "$count_durable" -eq 1 ]] && break
    sleep 1
done
[[ "$count_durable" -eq 1 ]] || fail "GitHub did not deliver all eight events into Braid"
count_started="$(date +%s)"
count_running=0
for _ in $(seq 1 15); do
    if [[ "$(agent_marker_count "$fixture_issue" "$count_marker")" -eq 1 ]]; then
        count_running=1
        break
    fi
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" '
        any(.transport.agent_groups[];
          .work_item_kind == "issue" and .work_item_number == $number and
          .session_lifecycle == "running" and .turn_lifecycle == "running")
    ' >/dev/null <<<"$status_payload"; then
        count_running=1
        break
    fi
    sleep 1
done
[[ "$count_running" -eq 1 ]] || \
    fail "eight-event batch was not accepted within 15 seconds of durable receipt"
for _ in $(seq 1 60); do
    [[ "$(agent_marker_count "$fixture_issue" "$count_marker")" -eq 1 ]] && break
    sleep 2
done
[[ "$(agent_marker_count "$fixture_issue" "$count_marker")" -eq 1 ]] || \
    fail "count-threshold turn did not publish exactly one Agent marker"
for comment_id in "${count_comments[@]}"; do
    has_reaction "$comment_id" eyes || fail "count-threshold comment did not receive eyes"
    for reaction in rocket +1 confused; do
        has_reaction "$comment_id" "$reaction" && \
            fail "count-threshold turn exposed request-like $reaction reaction"
    done
done
[[ "$(($(date +%s) - count_started))" -lt 90 ]] || \
    fail "count-threshold turn did not converge in the bounded window"

note "proving provider disconnect remains unknown and publishes Operational Status"
unknown_comment="$(gh api --method POST "repos/$repository/issues/$fixture_issue/comments" \
    -f body='@braid Unknown-outcome fixture: run a shell sleep for 60 seconds before any other action; do not publish a comment before the sleep finishes.' --jq '.id')"
for _ in $(seq 1 90); do
    has_reaction "$unknown_comment" eyes && has_reaction "$unknown_comment" rocket && break
    sleep 1
done
has_reaction "$unknown_comment" rocket || fail "unknown-outcome turn was not accepted"
provider_pid="$(pgrep -P "$runtime_pid" -f 'codex.*app-server' | head -1 || true)"
if [[ -z "$provider_pid" ]]; then
    provider_pid="$(pgrep -P "$runtime_pid" | head -1 || true)"
fi
[[ -n "$provider_pid" ]] || fail "cannot locate the real provider child process"
pkill -9 -P "$provider_pid" >/dev/null 2>&1 || true
kill -KILL "$provider_pid"
for _ in $(seq 1 60); do
    provider_health="$(curl -fsS "$health_url" 2>/dev/null | jq -r '.provider' || true)"
    [[ "$provider_health" == "unavailable" ]] && break
    sleep 1
done
[[ "${provider_health:-}" == "unavailable" ]] || fail "provider disconnect was not surfaced"
for _ in $(seq 1 60); do
    status_payload="$($binary status --config "$test_config" --json)"
    if jq -e --argjson number "$fixture_issue" '
        any(.transport.agent_groups[];
          .work_item_kind == "issue" and .work_item_number == $number and
          .session_lifecycle == "unknown" and .turn_lifecycle == "unknown")
    ' >/dev/null <<<"$status_payload"; then
        break
    fi
    sleep 1
done
jq -e --argjson number "$fixture_issue" '
    any(.transport.agent_groups[];
      .work_item_kind == "issue" and .work_item_number == $number and
      .session_lifecycle == "unknown" and .turn_lifecycle == "unknown")
' >/dev/null <<<"$status_payload" || fail "disconnect did not preserve an unknown turn"
has_reaction "$unknown_comment" rocket || fail "unknown turn did not retain rocket"
has_reaction "$unknown_comment" +1 && fail "unknown turn was reported successful"
has_reaction "$unknown_comment" confused && fail "unknown turn was reported failed"
for _ in $(seq 1 60); do
    operational_comments="$(gh api "repos/$repository/issues/$fixture_issue/comments" | \
        jq --arg actor "$app_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Operational Status")) and (.body | contains("Provider outcome unknown")))] | length')"
    [[ "$operational_comments" -eq 1 ]] && break
    sleep 1
done
[[ "${operational_comments:-0}" -eq 1 ]] || fail "provider unknown did not publish one Operational Status comment"

stop_process "$runtime_pid"
runtime_pid=""

note "proving a real accepted Codex turn can terminate failed and converge to confused"
readonly failure_root="$temporary_root/failure-runtime"
readonly failure_config="$temporary_root/failure.toml"
mkdir -p "$failure_root/state/backups"
awk \
    -v root="$failure_root" \
    -v database="$failure_root/state/braid.sqlite3" '
    /^\[runtime\]$/ { section="runtime"; print; next }
    /^\[\[profiles\]\]$/ { profile += 1; section="profile"; print; next }
    /^\[/ && $0 != "[runtime]" && $0 != "[[profiles]]" { section="other" }
    section == "runtime" && /^root = / { printf "root = \"%s\"\n", root; next }
    section == "runtime" && /^database = / { printf "database = \"%s\"\n", database; next }
    section == "runtime" && /^backups = / { printf "backups = \"%s/state/backups\"\n", root; next }
    profile == 1 && /^model = / { print "model = \"braid-intentionally-invalid-model\""; next }
    { print }
' "$test_config" > "$failure_config"
$binary migrate apply --config "$failure_config" >/dev/null
runtime_log="$temporary_root/failure-runtime.log"
BRAID_WEBHOOK_SECRET="$BRAID_WEBHOOK_SECRET" "$binary" serve \
    --config "$failure_config" >"$runtime_log" 2>&1 &
runtime_pid=$!
for _ in $(seq 1 120); do
    if curl -fsS "$health_url" 2>/dev/null | \
        jq -e '.ready == true and .provider == "connected"' >/dev/null; then
        break
    fi
    kill -0 "$runtime_pid" 2>/dev/null || fail "failure-fixture Braid exited during startup"
    sleep 1
done
failure_issue="$(gh api --method POST "repos/$repository/issues" \
    -f title="Braid Slice 3: failed terminal $(date -u +%Y%m%dT%H%M%SZ)" \
    -f body='Disposable real-provider failed-terminal fixture.' --jq '.number')"
failure_comment="$(gh api --method POST "repos/$repository/issues/$failure_issue/comments" \
    -f body='@braid Start the controlled real-provider failure turn.' --jq '.id')"
rocket_observed=0
for _ in $(seq 1 90); do
    has_reaction "$failure_comment" rocket && rocket_observed=1
    has_reaction "$failure_comment" confused && break
    sleep 1
done
[[ "$rocket_observed" -eq 1 ]] || fail "failed-terminal turn never exposed accepted rocket"
has_reaction "$failure_comment" confused || fail "real failed terminal did not converge to confused"
for _ in $(seq 1 30); do
    has_reaction "$failure_comment" rocket || break
    sleep 1
done
has_reaction "$failure_comment" rocket && fail "failed terminal retained stale rocket"
failure_agent_comments="$(gh api "repos/$repository/issues/$failure_issue/comments" | \
    jq --arg actor "$app_actor" '[.[] | select(.user.login == $actor and (.body | startswith("> **Braid Agent")))] | length')"
[[ "$failure_agent_comments" -eq 0 ]] || fail "failed provider turn fabricated an Agent comment"

stop_process "$runtime_pid"
runtime_pid=""
stop_process "$tunnel_pid"
tunnel_pid=""

jq -n \
    --arg repository "$repository" \
    --arg app_actor "$app_actor" \
    --arg agent_actor "$agent_actor" \
    --arg candidate "$candidate_version" \
    --arg candidate_sha256 "$candidate_sha256" \
    --arg activation "trusted @braid fallback (ordinary GitHub App is not assignable)" \
    --argjson issue "$fixture_issue" \
    --argjson failure_issue "$failure_issue" \
    '{
        verdict:"PASS",
        boundary:"real GitHub -> HTTP/2 Quick Tunnel -> packaged Braid -> real Codex app-server -> Agent gh comment",
        repository:$repository,
        activation:$activation,
        candidate:$candidate,
        candidate_sha256:$candidate_sha256,
        app_actor:$app_actor,
        agent_actor:$agent_actor,
        fixture_issue:$issue,
        failed_terminal_issue:$failure_issue,
        journeys:["trusted-mention-steer","ordinary-debounce","eight-event-threshold","provider-disconnect-unknown","real-provider-failed-terminal"]
    }'
