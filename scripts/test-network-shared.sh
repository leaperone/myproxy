#!/usr/bin/env bash
# Compile and run in-repo NetworkShared Swift tests (assert harness; no XCTest).
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/network-shared-tests}"
arch="$(uname -m)"
target="${arch}-apple-macosx14.0"
mkdir -p "$out"

shared_sources=(macos/NetworkShared/*.swift)
test_sources=(macos/NetworkSharedTests/*.swift)
if (( ${#shared_sources[@]} == 0 )); then
  echo "NetworkShared Swift sources are missing" >&2
  exit 1
fi
if (( ${#test_sources[@]} == 0 )); then
  echo "NetworkShared test sources are missing" >&2
  exit 1
fi

swiftc \
  -swift-version 6 \
  -Onone \
  -target "$target" \
  -module-name MyproxyNetworkSharedTests \
  -framework Foundation \
  "${shared_sources[@]}" \
  "${test_sources[@]}" \
  -o "$out/MyproxyNetworkSharedTests"

echo "built $out/MyproxyNetworkSharedTests"
"$out/MyproxyNetworkSharedTests"
