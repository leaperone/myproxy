#!/usr/bin/env bash
# Restart the local window on save, and push the same debug binaries to Air.
set -euo pipefail
cd "$(dirname "$0")/.."

cycle() {
  cargo build --bins
  if ! SKIP_BUILD=1 scripts/push-air.sh; then
    echo "air push failed; local window only" >&2
  fi
  exec target/debug/myproxy
}

if [[ "${1:-}" == "--cycle" ]]; then
  cycle
fi

if ! command -v watchexec >/dev/null 2>&1; then
  echo "watchexec not found; running once. brew install watchexec for restart-on-save." >&2
  cycle
fi

echo "watching → local window + ${SSH_HOST:-macbook-air}"
exec watchexec -r -e rs,toml --debounce 800ms -- "$PWD/scripts/dev.sh" --cycle
