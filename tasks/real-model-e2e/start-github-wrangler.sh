#!/bin/sh
set -eu

ROOT=/Volumes/WorkSSD/Development/braid-e2e/wrangler
WRANGLER=$ROOT/node_modules/wrangler/bin/wrangler.js

if [ "$#" -ne 1 ]; then
  echo "usage: $0 http://127.0.0.1:18180" >&2
  exit 2
fi

export TMPDIR=$ROOT/tmp
export XDG_CONFIG_HOME=$ROOT/config
export XDG_CACHE_HOME=$ROOT/cache
export WRANGLER_LOG_PATH=$ROOT/logs
export TUNNEL_TRANSPORT_PROTOCOL=http2

exec /opt/homebrew/bin/node "$WRANGLER" tunnel quick-start "$1" --log-level info
