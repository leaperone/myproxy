#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
ver="${SPARKLE_VERSION:-2.9.6}"
dest="resources/sparkle"
if [[ -d "$dest/Sparkle.framework" && -x "$dest/bin/generate_appcast" ]]; then
  echo "sparkle $ver already at $dest"
  exit 0
fi

mkdir -p "$dest"
asset="Sparkle-${ver}.tar.xz"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT

urls=(
  "https://github.com/sparkle-project/Sparkle/releases/download/${ver}/${asset}"
  "https://ghproxy.net/https://github.com/sparkle-project/Sparkle/releases/download/${ver}/${asset}"
  "https://mirror.ghproxy.com/https://github.com/sparkle-project/Sparkle/releases/download/${ver}/${asset}"
)

ok=0
for url in "${urls[@]}"; do
  echo "fetch $url"
  if curl -fsSL --connect-timeout 12 --max-time 90 "$url" -o "$tmp"; then
    ok=1
    break
  fi
done

if [[ "$ok" -ne 1 ]]; then
  echo "failed to download Sparkle ${ver}" >&2
  exit 1
fi

rm -rf "${dest}.extract"
mkdir -p "${dest}.extract"
tar -xJf "$tmp" -C "${dest}.extract"
# Tarball layout varies: either Sparkle.framework at root or nested.
framework="$(find "${dest}.extract" -name Sparkle.framework -type d -maxdepth 3 | head -n 1)"
bindir="$(find "${dest}.extract" -type d -name bin -maxdepth 3 | head -n 1)"
if [[ -z "$framework" ]]; then
  echo "Sparkle.framework missing from tarball" >&2
  exit 1
fi
rm -rf "$dest"
mkdir -p "$dest/bin"
cp -R "$framework" "$dest/Sparkle.framework"
if [[ -n "$bindir" ]]; then
  cp -R "$bindir"/. "$dest/bin/"
fi
rm -rf "${dest}.extract"
chmod +x "$dest/bin/"* 2>/dev/null || true
echo "installed Sparkle ${ver} → $dest"
