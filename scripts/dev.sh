#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if ! command -v watchexec >/dev/null 2>&1; then
  echo "watchexec not found; running once. brew install watchexec for restart-on-save." >&2
  exec cargo run --bin myproxy
fi
exec watchexec -r -e rs,toml -- cargo run --bin myproxy
