#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 scripts/check-xray-boundary.py
git diff --check
cargo test --locked --lib --features xray-channel xray:: -- --test-threads=1
echo "Xray channel unit tests passed"
