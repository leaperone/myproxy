#!/usr/bin/env bash
# Compile the Swift host static libraries that Rust links for System Extension control.
set -euo pipefail
cd "$(dirname "$0")/.."

out="${1:-target/network-host}"
arch="$(uname -m)"
target="${arch}-apple-macosx14.0"

host_sources=(macos/NetworkHost/*.swift)
if (( ${#host_sources[@]} == 0 )); then
  echo "Network Extension Swift sources are missing" >&2
  exit 1
fi

scripts/build-network-shared.sh "$out" "$target"

swiftc \
  -parse-as-library \
  -swift-version 6 \
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
