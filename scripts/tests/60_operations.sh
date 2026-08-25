#!/bin/sh
set -eu

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

source_config=${BRAID_CONFIG:?BRAID_CONFIG must point to the real acceptance config}
webhook_secret=${BRAID_WEBHOOK_SECRET:?BRAID_WEBHOOK_SECRET must match the dedicated App}
braid=${BRAID_BIN:-braid}
previous_braid=${BRAID_PREVIOUS_BIN:-}
repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-slice6.XXXXXX")
runtime_root="$temporary_root/runtime"
config="$temporary_root/braid.toml"
runtime_log="$temporary_root/runtime.log"
ingress_address=${BRAID_TEST_INGRESS:-127.0.0.1:18100}
health_address=${BRAID_TEST_HEALTH:-127.0.0.1:18101}
health_url="http://$health_address/healthz"
runtime_pid=
receiver_pid=
tunnel_runtime_log=
tunnel_scope=verified

stop_process() {
    pid=$1
    signal=${2:-TERM}
    [ -n "$pid" ] || return 0
    kill -0 "$pid" 2>/dev/null || return 0
    kill -"$signal" "$pid" 2>/dev/null || true
    for _ in $(seq 1 20); do
        kill -0 "$pid" 2>/dev/null || {
            wait "$pid" 2>/dev/null || true
            return 0
        }
        sleep 1
    done
    kill -KILL "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
}

cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    stop_process "$runtime_pid"
    stop_process "$receiver_pid"
    if [ "$status" -ne 0 ] && [ -f "$runtime_log" ]; then
        echo "Slice 6 runtime log follows" >&2
        tail -200 "$runtime_log" >&2
    fi
    if [ "$status" -ne 0 ] && [ -n "$tunnel_runtime_log" ] && [ -f "$tunnel_runtime_log" ]; then
        echo "Slice 6 tunnel runtime log follows" >&2
        tail -200 "$tunnel_runtime_log" >&2
    fi
    rm -rf "$temporary_root"
}
trap cleanup EXIT HUP INT TERM

command -v "$braid" >/dev/null 2>&1 || [ -x "$braid" ] || fail "Braid binary is unavailable"
command -v curl >/dev/null 2>&1 || fail "curl is unavailable"
command -v jq >/dev/null 2>&1 || fail "jq is unavailable"
command -v python3 >/dev/null 2>&1 || fail "python3 is required for the OTLP black-box helper"
[ -f "$source_config" ] || fail "acceptance config does not exist"
mkdir -p "$runtime_root/state/backups"

awk \
    -v root="$runtime_root" \
    -v database="$runtime_root/state/braid.sqlite3" \
    -v backups="$runtime_root/state/backups" \
    -v ingress="$ingress_address" \
    -v health="$health_address" '
    /^\[runtime\]$/ { section="runtime"; print; next }
    /^\[server\]$/ { section="server"; print; next }
    /^\[telemetry\]$/ { section="telemetry"; print; next }
    /^\[/ && $0 != "[runtime]" && $0 != "[server]" && $0 != "[telemetry]" { section="other" }
    section == "runtime" && /^root = / { printf "root = \"%s\"\n", root; next }
    section == "runtime" && /^database = / { printf "database = \"%s\"\n", database; next }
    section == "runtime" && /^backups = / { printf "backups = \"%s\"\n", backups; next }
    section == "runtime" && /^auto_migrate = / { print "auto_migrate = false"; next }
    section == "server" && /^ingress = / { printf "ingress = \"%s\"\n", ingress; next }
    section == "server" && /^health = / { printf "health = \"%s\"\n", health; next }
    section == "telemetry" && /^endpoint = / { print "endpoint = \"http://127.0.0.1:43219\""; next }
    section == "telemetry" && /^sample_ratio = / { print "sample_ratio = 0.10"; next }
    section == "telemetry" && /^incident_mode = / { print "incident_mode = false"; next }
    { print }
' "$source_config" > "$config"

"$braid" migrate apply --config "$config" >/dev/null
archive=$(BRAID_DIST_DIR="$temporary_root/dist" "$repository_root/scripts/package.sh")
"$repository_root/scripts/tests/00_clean_install.sh" "$archive"

if [ -z "$previous_braid" ] || [ ! -x "$previous_braid" ]; then
    fail "BRAID_PREVIOUS_BIN must name the declared schema-compatible rollback binary"
fi
"$previous_braid" status --config "$config" --json | \
    jq -e '.database.schema_version == .database.supported_schema' >/dev/null || \
    fail "the declared rollback binary cannot read the migrated database"

start_transport() {
    BRAID_WEBHOOK_SECRET="$webhook_secret" "$braid" serve --config "$config" --transport-only \
        >"$runtime_log" 2>&1 &
    runtime_pid=$!
    for _ in $(seq 1 90); do
        if curl -fsS "$health_url" 2>/dev/null | jq -e '.ready == true' >/dev/null; then
            return 0
        fi
        kill -0 "$runtime_pid" 2>/dev/null || fail "transport runtime exited before readiness"
        sleep 1
    done
    fail "transport runtime did not become ready"
}

start_transport
stop_process "$runtime_pid" TERM
runtime_pid=
"$braid" status --config "$config" --json | jq -e '.transport.owner == null' >/dev/null || \
    fail "SIGTERM did not release the runtime owner lease"

start_transport
killed_pid=$runtime_pid
kill -KILL "$killed_pid"
wait "$killed_pid" 2>/dev/null || true
runtime_pid=
if BRAID_WEBHOOK_SECRET="$webhook_secret" "$braid" serve --config "$config" --transport-only \
    >"$temporary_root/fenced-restart.log" 2>&1; then
    fail "forced shutdown allowed a second owner before lease expiry"
fi
grep -q 'another Braid runtime owns' "$temporary_root/fenced-restart.log" || \
    fail "forced restart did not expose owner fencing"
sleep 31
start_transport
stop_process "$runtime_pid" TERM
runtime_pid=

if [ "${BRAID_TEST_SKIP_TUNNEL:-0}" != "1" ]; then
    tunnel_runtime_log="$temporary_root/tunnel-runtime.log"
    BRAID_WEBHOOK_SECRET="$webhook_secret" "$braid" serve --config "$config" \
        --transport-only --tunnel >"$tunnel_runtime_log" 2>&1 &
    runtime_pid=$!
    public_webhook=
    for _ in $(seq 1 900); do
        health=$(curl -fsS "$health_url" 2>/dev/null || true)
        if printf '%s' "$health" | jq -e \
            '.ready == true and .tunnel == "connected" and .webhook_url != null' >/dev/null 2>&1; then
            public_webhook=$(printf '%s' "$health" | jq -er .webhook_url)
            break
        fi
        kill -0 "$runtime_pid" 2>/dev/null || fail "tunnel runtime exited before readiness"
        sleep 1
    done
    [ -n "$public_webhook" ] || fail "Braid did not establish a verified internal Quick Tunnel"
    tunnel_child=$(pgrep -P "$runtime_pid" | head -1 || true)
    [ -n "$tunnel_child" ] || fail "Braid Quick Tunnel child was not observable"
    kill -TERM "$tunnel_child"
    for _ in $(seq 1 60); do
        health=$(curl -fsS "$health_url" 2>/dev/null || true)
        if printf '%s' "$health" | jq -e \
            --arg public "$public_webhook" '
            .ready == true and .tunnel == "unavailable" and
            .webhook_url != $public and
            (.last_error | contains("prior App webhook was restored"))' >/dev/null 2>&1; then
            break
        fi
        kill -0 "$runtime_pid" 2>/dev/null || fail "runtime exited with its Quick Tunnel"
        sleep 1
    done
    printf '%s' "$health" | jq -e \
        '.ready == true and .tunnel == "unavailable" and
         (.last_error | contains("prior App webhook was restored"))' >/dev/null || \
        fail "Quick Tunnel death did not repair the App webhook and preserve runtime health"
    stop_process "$runtime_pid" TERM
    runtime_pid=
else
    tunnel_scope=unavailable
fi

sample_config="$temporary_root/sample.toml"
cp "$config" "$sample_config"
capture="$temporary_root/otel-sample.bin"
python3 "$repository_root/scripts/tests/otel_receiver.py" \
    --port 43219 --minimum-requests 3 --idle-seconds 2 --deadline-seconds 180 \
    --output "$capture" &
receiver_pid=$!
sleep 1
sampled=0
probes=0
for index in $(seq 1 120); do
    result=$("$braid" telemetry probe --config "$sample_config" \
        --marker "BRAID_SLICE6_SAMPLE_$index" --json)
    probes=$index
    printf '%s' "$result" | jq -e '.sampled == .payload_emitted' >/dev/null || \
        fail "trace sampling orphaned a payload child"
    if printf '%s' "$result" | jq -e '.sampled == true' >/dev/null; then
        sampled=$((sampled + 1))
    fi
    if [ "$probes" -ge 30 ] && [ "$sampled" -gt 0 ] && [ "$sampled" -lt "$probes" ]; then
        break
    fi
done
wait "$receiver_pid"
receiver_pid=
[ "$sampled" -gt 0 ] && [ "$sampled" -lt "$probes" ] || \
    fail "10% sampling did not retain a bounded subset ($sampled/$probes)"
grep -a -q 'PATH /v1/traces' "$capture" || fail "sampled traces did not reach OTLP"

incident_config="$temporary_root/incident.toml"
awk '
    /^incident_mode = / { print "incident_mode = true"; next }
    /^endpoint = / { print "endpoint = \"http://127.0.0.1:43220\""; next }
    { print }
' "$sample_config" > "$incident_config"
incident_capture="$temporary_root/otel-incident.bin"
python3 "$repository_root/scripts/tests/otel_receiver.py" \
    --port 43220 --minimum-requests 3 --deadline-seconds 30 --output "$incident_capture" &
receiver_pid=$!
sleep 1
incident=$("$braid" telemetry probe --config "$incident_config" \
    --marker BRAID_SLICE6_INCIDENT --json)
printf '%s' "$incident" | jq -e \
    '.sampled == true and .payload_emitted == true and
     .exporter.configured_sample_ratio == 0.1 and
     .exporter.effective_sample_ratio == 1.0 and .exporter.incident_mode == true' \
    >/dev/null || fail "incident mode did not force complete sampling"
wait "$receiver_pid"
receiver_pid=
grep -a -q 'BRAID_SLICE6_INCIDENT' "$incident_capture" || \
    fail "incident payload evidence did not reach OTLP"

echo "PASS: Slice 6 packaging, migration/rollback, owner fencing, SIGTERM, and OTel convergence"
echo "candidate=$($braid --version) sampled=$sampled/$probes tunnel=$tunnel_scope"
echo "UNPROVEN HERE: active-turn restart and uncertain remote write; active restart is covered by the real Slice 5/6 campaign"
