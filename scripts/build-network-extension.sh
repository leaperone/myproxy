#!/usr/bin/env bash
# Compile the System Extension executable (NETransparentProxyProvider).
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/network-extension}"
arch="$(uname -m)"
target="${arch}-apple-macosx14.0"

extension_sources=(macos/NetworkExtension/*.swift)
if (( ${#extension_sources[@]} == 0 )); then
  echo "Network Extension Swift sources are missing" >&2
  exit 1
fi

scripts/build-network-shared.sh "$out" "$target"

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
