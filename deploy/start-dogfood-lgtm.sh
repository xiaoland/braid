#!/bin/bash
# Pinned otel-lgtm:0.11.0 has no retention env knobs for Tempo/Prometheus.
# Change only its container-local defaults, never the source verification files.
set -eu
cd /otel-lgtm
if ! grep -q -- '--storage.tsdb.retention.time=7d' run-prometheus.sh; then
  sed -i '/--storage.tsdb.path=/a\	--storage.tsdb.retention.time=7d \\' run-prometheus.sh
fi
if ! grep -q '^compactor:' tempo-config.yaml; then
  cat >> tempo-config.yaml <<'EOF'

compactor:
  compaction:
    block_retention: 168h
EOF
fi
exec ./run-all.sh
