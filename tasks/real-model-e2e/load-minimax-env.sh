#!/bin/sh
set -eu

env_file=${BRAID_E2E_ENV_FILE:-/Volumes/WorkSSD/Development/agentic26/.env}

read_value() {
    key=$1
    awk -F= -v wanted="$key" '
        $1 == wanted {
            value = substr($0, index($0, "=") + 1)
            gsub(/[[:space:]\047\042]/, "", value)
            print value
            exit
        }
    ' "$env_file"
}

MINIMAX_API_KEY=$(read_value MINIMAX_API_KEY)
MINIMAX_BASE_URL=$(read_value MINIMAX_BASE_URL)
[ -n "$MINIMAX_API_KEY" ] || { echo "MINIMAX_API_KEY is empty" >&2; exit 1; }
[ -n "$MINIMAX_BASE_URL" ] || { echo "MINIMAX_BASE_URL is empty" >&2; exit 1; }
export MINIMAX_API_KEY MINIMAX_BASE_URL

if [ "$#" -eq 0 ]; then
    echo "usage: load-minimax-env.sh <command> [args...]" >&2
    exit 2
fi
exec "$@"
