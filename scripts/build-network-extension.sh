#!/usr/bin/env bash
# Compile the System Extension executable (NETransparentProxyProvider).
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/network-extension}"
arch="$(uname -m)"
target="${arch}-apple-macosx14.0"
mkdir -p "$out"

shared_sources=(macos/NetworkShared/*.swift)
extension_sources=(macos/NetworkExtension/*.swift)
if (( ${#shared_sources[@]} == 0 )) || (( ${#extension_sources[@]} == 0 )); then
  echo "Network Extension Swift sources are missing" >&2
  exit 1
fi

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
  "${shared_sources[@]}" \
  -emit-module-path "$out/MyproxyNetworkShared.swiftmodule" \
  -o "$out/libMyproxyNetworkShared.a"

swiftc \
  -swift-version 6 \
  -O \
  -whole-module-optimization \
  -target "$target" \
  -module-name MyproxyNetworkExtension \
  -framework Network \
  -framework NetworkExtension \
  -framework Security \
  -lbsm \
  -I "$out" \
  -L "$out" \
  -lMyproxyNetworkShared \
  "${extension_sources[@]}" \
  -o "$out/MyproxyNetworkExtension"

echo "built $out/MyproxyNetworkExtension"
