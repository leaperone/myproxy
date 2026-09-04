#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p resources/mihomo
ver="${MIHOMO_VERSION:-v1.19.30}"
asset="mihomo-darwin-arm64-${ver}.gz"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

urls=(
  "https://github.com/MetaCubeX/mihomo/releases/download/${ver}/${asset}"
  "https://ghproxy.net/https://github.com/MetaCubeX/mihomo/releases/download/${ver}/${asset}"
  "https://mirror.ghproxy.com/https://github.com/MetaCubeX/mihomo/releases/download/${ver}/${asset}"
)

ok=0
for url in "${urls[@]}"; do
  echo "fetch $url"
  if curl -fsSL --connect-timeout 12 --max-time 90 "$url" -o "$tmp"; then
    gzip -dc "$tmp" > resources/mihomo/mihomo
    ok=1
    break
  fi
done

if [[ "$ok" -ne 1 ]]; then
  echo "failed to download mihomo ${ver}" >&2
  exit 1
fi

chmod +x resources/mihomo/mihomo
echo "installed $ver → resources/mihomo/mihomo"
./resources/mihomo/mihomo -v || true
