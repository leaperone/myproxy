#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:?output directory required}"
target="${2:?target triple required}"
mkdir -p "$out"

swiftc \
  -parse-as-library \
  -swift-version 6 \
  -O \
  -whole-module-optimization \
  -target "$target" \
  -emit-module \
  -emit-library \
  -static \
  -module-name MyproxyNetworkShared \
  macos/NetworkShared/*.swift \
  -emit-module-path "$out/MyproxyNetworkShared.swiftmodule" \
  -o "$out/libMyproxyNetworkShared.a"
