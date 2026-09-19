#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

python3 scripts/check-xray-boundary.py
git diff --check
cargo test --lib xray::tests
echo "Xray channel unit tests passed"
