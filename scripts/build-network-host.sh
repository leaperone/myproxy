#!/usr/bin/env bash
# Compile the Swift host static libraries that Rust links for System Extension control.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/network-host}"
arch="$(uname -m)"
target="${arch}-apple-macosx14.0"
mkdir -p "$out"
swift_flags=(-swift-version 6)
if [[ "${MYPROXY_XRAY_CHANNEL:-0}" == "1" ]]; then
  swift_flags+=(-D MYPROXY_XRAY)
fi

shared_sources=(macos/NetworkShared/*.swift)
host_sources=(macos/NetworkHost/*.swift)
if (( ${#shared_sources[@]} == 0 )) || (( ${#host_sources[@]} == 0 )); then
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
  -parse-as-library \
  -O \
  -whole-module-optimization \
  -target "$target" \
  -emit-library \
  -static \
  -module-name MyproxyNetworkHost \
  -framework Foundation \
  -framework NetworkExtension \
  -framework SystemExtensions \
  -I "$out" \
  -L "$out" \
  -lMyproxyNetworkShared \
  "${host_sources[@]}" \
  -o "$out/libMyproxyNetworkHost.a"

echo "built $out/libMyproxyNetworkHost.a"
