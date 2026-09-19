#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# This script is intentionally separate from fetch-mihomo.sh and is never
# called by the stable or nightly release workflows.
version="${XRAY_VERSION:-v26.9.9}"
asset=""
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Darwin-arm64*) asset="Xray-macos-arm64-v8a.zip" ;;
  Darwin-x86_64) asset="Xray-macos-64.zip" ;;
  Linux-aarch64|Linux-arm64) asset="Xray-linux-arm64-v8a.zip" ;;
  Linux-x86_64) asset="Xray-linux-64.zip" ;;
  *) echo "unsupported Xray host: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

mkdir -p resources/xray
tmp="$(mktemp -t myproxy-xray.XXXXXX.zip)"
trap 'rm -f "$tmp"' EXIT
urls=(
  "https://github.com/XTLS/Xray-core/releases/download/${version}/${asset}"
  "https://ghproxy.net/https://github.com/XTLS/Xray-core/releases/download/${version}/${asset}"
  "https://mirror.ghproxy.com/https://github.com/XTLS/Xray-core/releases/download/${version}/${asset}"
)
for url in "${urls[@]}"; do
  if curl -fL --retry 2 --connect-timeout 8 --max-time 60 "$url" -o "$tmp"; then
    unzip -p "$tmp" xray > resources/xray/xray
    chmod +x resources/xray/xray
    echo "installed Xray ${version} → resources/xray/xray"
    resources/xray/xray version || true
    exit 0
  fi
done
echo "failed to download Xray ${version}" >&2
exit 1
