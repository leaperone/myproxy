#!/usr/bin/env bash
# Compile the System Extension executable (NETransparentProxyProvider).
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/network-extension}"
arch="$(uname -m)"
target="${arch}-apple-macosx14.0"
mkdir -p "$out"
swift_flags=(-swift-version 6)
if [[ "${MYPROXY_XRAY_CHANNEL:-0}" == "1" ]]; then
  swift_flags+=(-D MYPROXY_XRAY)
fi

shared_sources=(macos/NetworkShared/*.swift)
extension_sources=(macos/NetworkExtension/*.swift)
if (( ${#shared_sources[@]} == 0 )) || (( ${#extension_sources[@]} == 0 )); then
  echo "Network Extension Swift sources are missing" >&2
  exit 1
fi

swiftc \
  "${swift_flags[@]}" \
  -parse-as-library \
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
  "${swift_flags[@]}" \
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
