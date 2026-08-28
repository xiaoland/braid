#!/bin/sh
set -eu

repository_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
temporary_root=$(mktemp -d "${TMPDIR:-/tmp}/braid-instances.XXXXXX")
trap 'rm -rf "$temporary_root"' EXIT HUP INT TERM

archive=${1:-}
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

user_home="$temporary_root/home/.braid"
mkdir -p "$user_home/instances/first/state" \
    "$user_home/instances/second/state" \
    "$user_home/secrets"

printf '%s\n' 'provider_api_key = "fake-deepseek"' > "$user_home/secrets/deepseek.toml"
printf '%s\n' 'provider_api_key = "fake-openai"' > "$user_home/secrets/openai.toml"

cat > "$user_home/instances/first/github-app.pem" <<EOF
-----BEGIN RSA PRIVATE KEY-----
diagnostic
-----END RSA PRIVATE KEY-----
EOF
cp "$user_home/instances/first/github-app.pem" "$user_home/instances/second/github-app.pem"

# Ports are deliberately overlapping to exercise doctor cross-instance detection.
write_instance_config() {
    key=$1
    app_id=$2
    port=$3
    cat > "$user_home/instances/$key/config.toml" <<EOF
schema_version = 2

[instance]
key = "$key"

[runtime]
root = "$user_home/instances/$key/state"
[github]
app_id = $app_id
repository = "$key/braid"
handle = "braid"
api_version = "2022-11-28"
private_key_file = "$user_home/instances/$key/github-app.pem"
webhook_secret_environment = "BRAID_WEBHOOK_SECRET"
projects_v2_enabled = false
[scheduler]
quiet_seconds = 30
event_threshold = 8
reconciliation_seconds = 60
[[runtimes]]
adapter_type = "pi"
version = "0.84.3"
executable = "/usr/bin/false"
home = "$user_home/instances/$key/provider"
[[llm_providers]]
id = "deepseek"
protocol = "openai-compatible"
api_key_file = "$user_home/secrets/deepseek.toml"
api_key_environment = "DEEPSEEK_API_KEY"
[[llm_providers.models]]
model_id = "deepseek-chat"
input_cost = 0.0
output_cost = 0.0
cache_input_cost = 0.0
[tools]
git = "/usr/bin/git"
gh = "/usr/bin/false"
wrangler = "/usr/bin/false"
[server]
ingress = "127.0.0.1:$port"
health = "127.0.0.1:$((port + 1))"
[telemetry]
endpoint = "http://127.0.0.1:43189"
sample_ratio = 0.0
incident_mode = false
export_timeout_seconds = 5
service_name = "braid"
log_format = "text"
[[profiles]]
id = "default"
display_name = "Braid"
tags = ["issue", "pr"]
adapter_type = "pi"
adapter_version = "0.84.3"
provider = "deepseek"
model = "deepseek-chat"
reasoning = "high"
user_instructions = "x"
workspace = "$user_home/instances/$key/workspace"
status_surfaces = ["issue", "pr"]
github_context_soft_ratio = 0.5
github_context_hard_bytes = 1000
[profile_selection]
default_pr_profile = "default"
EOF
}

write_instance_config first 111111 18090
write_instance_config second 222222 18090

cat > "$user_home/registry.toml" <<EOF
schema_version = 1
default_instance = "first"

[[instances]]
key = "first"
home = "instances/first"
github_app_id = 111111
repository = "first/braid"

[[instances]]
key = "second"
home = "instances/second"
github_app_id = 222222
repository = "second/braid"
EOF

run() {
    env -i PATH="$package_root/bin:/usr/bin:/bin" HOME="$temporary_root/home" BRAID_WEBHOOK_SECRET=diagnostic "$@"
}

# Default instance resolution.
run "$braid" config check --instance first > "$temporary_root/first.out"

# Explicit instance selection.
run "$braid" config check --instance second > "$temporary_root/second.out"

# Env-based instance selection.
run env BRAID_INSTANCE=second "$braid" config check > "$temporary_root/second-env.out"

# BRAID_INSTANCE_HOME override.
run env BRAID_INSTANCE_HOME="$user_home/instances/second" "$braid" config check > "$temporary_root/second-home.out"

# --config bypasses registry.
run "$braid" config check --config "$user_home/instances/first/config.toml" > "$temporary_root/first-bypass.out"

# Doctor detects the overlapping ingress/health ports across instances.
if run "$braid" doctor --json > "$temporary_root/doctor.json" 2>&1; then
    echo "doctor unexpectedly accepted overlapping ports" >&2
    exit 1
fi
/usr/bin/grep -qE 'duplicate|collision' "$temporary_root/doctor.json" || {
    echo "--- doctor.json ---" >&2
    cat "$temporary_root/doctor.json" >&2
    echo "doctor did not report port conflict" >&2
    exit 1
}

echo "OK: instance resolution and registry port cross-check"
