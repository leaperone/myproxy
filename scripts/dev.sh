#!/usr/bin/env bash
# Restart the local window on save. Does not push binaries to Air.
set -euo pipefail
cd "$(dirname "$0")/.."

cycle() {
  cargo build --bins
  exec target/debug/myproxy
}

if [[ "${1:-}" == "--cycle" ]]; then
  cycle
fi

if ! command -v watchexec >/dev/null 2>&1; then
  echo "watchexec not found; running once. brew install watchexec for restart-on-save." >&2
  cycle
fi

echo "watching → local window"
exec watchexec -r -e rs,toml --debounce 800ms -- "$PWD/scripts/dev.sh" --cycle
