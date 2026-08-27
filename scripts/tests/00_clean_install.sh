#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-clean-install.XXXXXX")
python3_executable=$(command -v python3 || true)
trap 'rm -rf "$temporary_root"' EXIT HUP INT TERM

archive=${1:-}
if [ -z "$archive" ]; then
    archive=$(BRAID_DIST_DIR="$temporary_root/dist" "$repository_root/scripts/package.sh")
fi
case "$archive" in
    /*) ;;
    *) archive="$repository_root/$archive" ;;
esac
(cd "$(dirname "$archive")" && /usr/bin/shasum -a 256 -c "$(basename "$archive").sha256")

install_root="$temporary_root/install"
mkdir -p "$install_root"
tar -C "$install_root" -xzf "$archive"
package_root=$(find "$install_root" -mindepth 1 -maxdepth 1 -type d -name 'braid-*' | head -1)
braid="$package_root/bin/braid"
runtime="$temporary_root/runtime"
mkdir -p "$runtime/state/backups" "$runtime/provider" "$runtime/workspace"
private_key="$runtime/github-app.pem"
secrets_file="$runtime/secrets.toml"
printf '%s\n' 'diagnostic placeholder; no GitHub API call is made in Slice 0' > "$private_key"
touch "$secrets_file"

write_config() {
    config_path=$1
    database_path=$2
    sample_ratio=$3
    port=$4
    cat > "$config_path" <<EOF
schema_version = 2
[runtime]
root = "$runtime"
database = "$database_path"
backups = "$runtime/state/backups"
auto_migrate = false
[github]
app_id = 1
repository = "xiaoland/braid"
handle = "braid"
api_version = "2022-11-28"
private_key_file = "$private_key"
webhook_secret_environment = "BRAID_WEBHOOK_SECRET"
projects_v2_enabled = false
[scheduler]
quiet_seconds = 30
event_threshold = 8
reconciliation_seconds = 60
[[runtimes]]
adapter_type = "codex"
version = "codex-cli 0.147.0-alpha.6.5"
executable = "/usr/bin/false"
home = "$runtime/provider"
stable_schema_sha256 = "7d79fe309dd7520843459070f3884ecf0e39cee2620c1c49aad6efb4eca76ecb"
experimental_schema_sha256 = "a14d4878fe7b8cdd31059dbca11d7167d8cfd06effa2f7991b5364439063a5c8"
[[llm_providers]]
id = "openai"
protocol = "openai"
api_key_file = "$secrets_file"
[[llm_providers.models]]
model_id = "gpt-5.6-sol"
input_cost = 0.0
output_cost = 0.0
cache_input_cost = 0.0
[tools]
git = "/usr/bin/git"
gh = "/usr/bin/false"
wrangler = "/usr/bin/false"
[server]
ingress = "127.0.0.1:18080"
health = "127.0.0.1:18081"
[telemetry]
endpoint = "http://127.0.0.1:$port"
sample_ratio = $sample_ratio
incident_mode = false
export_timeout_seconds = 5
service_name = "braid-clean-install"
log_format = "text"
[[profiles]]
id = "issue-codex"
display_name = "Issue Codex"
tags = ["issue"]
provider = "openai"
model = "gpt-5.6-sol"
reasoning = "high"
adapter_type = "codex"
adapter_version = "codex-cli 0.147.0-alpha.6.5"
user_instructions = "Use the Issue as working memory."
workspace = "$runtime/workspace"
status_surfaces = ["issue"]
github_context_soft_ratio = 0.80
github_context_hard_bytes = 524288
[[profiles]]
id = "pr-codex"
display_name = "PR Codex"
tags = ["pr", "implementation"]
provider = "openai"
model = "gpt-5.6-sol"
reasoning = "high"
adapter_type = "codex"
adapter_version = "codex-cli 0.147.0-alpha.6.5"
user_instructions = "Use linked Issues and the PR as working memory."
workspace = "$runtime/workspace"
status_surfaces = ["pr"]
github_context_soft_ratio = 0.80
github_context_hard_bytes = 524288
[profile_selection]
default_pr_profile = "pr-codex"
EOF
}

config="$temporary_root/braid.toml"
database="$runtime/state/braid.sqlite3"
write_config "$config" "$database" 1.0 43189
/usr/bin/sqlite3 "$database" 'PRAGMA application_id=1112688964;'
clean_path="$package_root/bin:/bin"

run_clean() {
    env -i PATH="$clean_path" HOME="$temporary_root/home" BRAID_WEBHOOK_SECRET=diagnostic "$@"
}

run_clean "$braid" --version
run_clean "$braid" config check --config "$config"
run_clean "$braid" migrate plan --config "$config"
run_clean "$braid" migrate apply --config "$config"
test "$(find "$runtime/state/backups" -type f -name '*.sqlite3' | wc -l | tr -d ' ')" = "1"
run_clean "$braid" profile inspect --config "$config" --profile pr-codex
run_clean "$braid" status --config "$config"

if run_clean "$braid" doctor --config "$config" --json > "$temporary_root/doctor.json" 2>&1; then
    echo "doctor unexpectedly accepted unavailable diagnostic dependencies" >&2
    exit 1
fi
/usr/bin/grep -q '"ready": false' "$temporary_root/doctor.json"
/usr/bin/grep -q 'Codex app-server' "$temporary_root/doctor.json"

schema=$(run_clean "$braid" status --config "$config" --json | /usr/bin/sed -n 's/.*"schema_version": \([0-9][0-9]*\).*/\1/p')
test "$schema" = "1"

# v1 fixture: intentionally a v1 DB that must remain compatible after migration
v1="$runtime/state/v1.sqlite3"
/usr/bin/sqlite3 "$v1" < "$repository_root/migrations/0001_initial.sql"
v1_checksum=$(/usr/bin/shasum -a 256 "$repository_root/migrations/0001_initial.sql" | /usr/bin/awk '{print $1}')
/usr/bin/sqlite3 "$v1" "INSERT INTO schema_migrations VALUES (1,'initial','$v1_checksum','fixture');"
/usr/bin/sqlite3 "$v1" "
INSERT INTO repositories(node_id,name_with_owner,observed_at)
VALUES ('REPOSITORY_NODE','xiaoland/braid','fixture');
INSERT INTO work_items(node_id,repository_node_id,kind,number,state,observed_at)
VALUES ('ISSUE_NODE','REPOSITORY_NODE','issue',9001,'OPEN','fixture'),
       ('PR_NODE','REPOSITORY_NODE','pr',9002,'OPEN','fixture');
INSERT INTO associations(issue_node_id,pr_node_id,source,observed_version,active)
VALUES ('ISSUE_NODE','PR_NODE','native','fixture',1);
INSERT INTO canonical_objects(
  node_id,work_item_node_id,object_kind,version,digest,lifecycle,observed_at
) VALUES (
  'COMMENT_NODE','ISSUE_NODE','issue_comment','fixture',
  '0000000000000000000000000000000000000000000000000000000000000000',
  'active','fixture'
);"
v1_config="$temporary_root/v1.toml"
write_config "$v1_config" "$v1" 1.0 43189
backup_count_before=$(find "$runtime/state/backups" -type f -name '*.sqlite3' | wc -l | tr -d ' ')
run_clean "$braid" migrate apply --config "$v1_config"
backup_count_after=$(find "$runtime/state/backups" -type f -name '*.sqlite3' | wc -l | tr -d ' ')
test "$backup_count_after" = "$backup_count_before"
v1_schema=$(run_clean "$braid" status --config "$v1_config" --json | /usr/bin/sed -n 's/.*"schema_version": \([0-9][0-9]*\).*/\1/p')
test "$v1_schema" = "1"
/usr/bin/sqlite3 "$v1" \
    "SELECT 1 FROM associations WHERE issue_node_id='ISSUE_NODE' AND pr_node_id='PR_NODE' AND active=1;" \
    | /usr/bin/grep -q '^1$'
/usr/bin/sqlite3 "$v1" \
    "SELECT 1 FROM canonical_objects WHERE node_id='COMMENT_NODE' AND lifecycle='active';" \
    | /usr/bin/grep -q '^1$'
test "$(/usr/bin/sqlite3 "$v1" "SELECT count(*) FROM issue_context_sources;")" = "0"
if /usr/bin/sqlite3 "$v1" "SELECT issue_content_digest FROM associations LIMIT 1;" \
    >"$temporary_root/removed-column.out" 2>&1; then
    echo "schema 11 retained the obsolete association digest column" >&2
    exit 1
fi

newer="$runtime/state/newer.sqlite3"
/usr/bin/sqlite3 "$newer" 'CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, name TEXT NOT NULL, checksum TEXT NOT NULL, applied_at TEXT NOT NULL); INSERT INTO schema_migrations VALUES (12,"future","0000000000000000000000000000000000000000000000000000000000000000","future");'
newer_config="$temporary_root/newer.toml"
write_config "$newer_config" "$newer" 1.0 43189
if run_clean "$braid" migrate plan --config "$newer_config" > "$temporary_root/newer.out" 2>&1; then
    echo "schema-newer fixture was accepted unexpectedly" >&2
    exit 1
fi
/usr/bin/grep -q 'schema 12 is newer' "$temporary_root/newer.out"

foreign="$runtime/state/foreign.sqlite3"
/usr/bin/sqlite3 "$foreign" 'CREATE TABLE prototype_state(id INTEGER PRIMARY KEY);'
foreign_config="$temporary_root/foreign.toml"
write_config "$foreign_config" "$foreign" 1.0 43189
if run_clean "$braid" migrate plan --config "$foreign_config" > "$temporary_root/foreign.out" 2>&1; then
    echo "foreign database fixture was accepted unexpectedly" >&2
    exit 1
fi
/usr/bin/grep -q 'not an empty or Braid Rust database' "$temporary_root/foreign.out"

if [ -n "$python3_executable" ] && [ -x "$python3_executable" ]; then
    capture_one="$temporary_root/otel-one.bin"
    "$python3_executable" "$repository_root/scripts/tests/otel_receiver.py" --port 43189 --minimum-requests 3 --output "$capture_one" &
    receiver=$!
    sleep 1
    run_clean "$braid" telemetry probe --config "$config" --marker BRAID_OTEL_FULL_PAYLOAD_PROBE
    wait "$receiver"
    /usr/bin/grep -a -q 'BRAID_OTEL_FULL_PAYLOAD_PROBE' "$capture_one"
    /usr/bin/grep -a -q 'PATH /v1/traces' "$capture_one"

    capture_zero="$temporary_root/otel-zero.bin"
    write_config "$config" "$database" 0.0 43189
    "$python3_executable" "$repository_root/scripts/tests/otel_receiver.py" --port 43189 --minimum-requests 1 --output "$capture_zero" &
    receiver=$!
    sleep 1
    run_clean "$braid" telemetry probe --config "$config" --marker BRAID_OTEL_MUST_NOT_EXPORT
    wait "$receiver"
    if /usr/bin/grep -a -q 'PATH /v1/traces' "$capture_zero"; then
        echo "ratio 0 exported an orphan trace" >&2
        exit 1
    fi
else
    echo "UNAVAILABLE: python3 is required only for the local OTLP capture helper" >&2
    exit 1
fi

echo "clean-install Slice 0 diagnostics passed"
