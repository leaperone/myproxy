#!/usr/bin/env bash
# Rebuild on save and hot-reload the Air app. Strategy.json on Air is left alone.
set -euo pipefail
cd "$(dirname "$0")/.."
export SSH_HOST="${SSH_HOST:-macbook-air}"
if ! command -v watchexec >/dev/null 2>&1; then
  echo "watchexec not found; pushing once. brew install watchexec to watch." >&2
  exec scripts/push-air.sh
fi
echo "watching src/ → $SSH_HOST"
exec watchexec -e rs,toml --watch src --watch Cargo.toml --debounce 800ms -- scripts/push-air.sh
