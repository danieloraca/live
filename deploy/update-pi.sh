#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

git pull --ff-only
cargo test
cargo build --release
sudo systemctl restart live.service
systemctl is-active live.service

for attempt in {1..10}; do
  if curl --fail --silent --show-error --max-time 2 \
    http://127.0.0.1:9999/api/status >/dev/null 2>&1; then
    echo "Pi Status is responding at http://127.0.0.1:9999/"
    exit 0
  fi
  sleep 1
done

echo "live.service restarted, but the status API did not respond." >&2
exit 1
