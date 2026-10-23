#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# This script is intentionally separate from fetch-mihomo.sh and is never
# called by the stable or nightly release workflows.
version="${XRAY_VERSION:-v26.10.21}"
asset=""
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64|Darwin-arm64*) asset="Xray-macos-arm64-v8a.zip" ;;
  Darwin-x86_64) asset="Xray-macos-64.zip" ;;
  Linux-aarch64|Linux-arm64) asset="Xray-linux-arm64-v8a.zip" ;;
  Linux-x86_64) asset="Xray-linux-64.zip" ;;
  *) echo "unsupported Xray host: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

# Pinned SHA2-256 from official GitHub release .dgst (v26.10.21).
expected_sha256() {
  case "${version}:${asset}" in
    v26.10.21:Xray-macos-arm64-v8a.zip) echo "926cc55fda2a8757dabe4f22dc98772f218ace025f101bd0b68aee232e59d5dc" ;;
    v26.10.21:Xray-macos-64.zip) echo "8e5169b429e76ce5f550c8294db465bc259f30c158ba900aa40802b76ad2f66e" ;;
    v26.10.21:Xray-linux-arm64-v8a.zip) echo "d1304946c486d98e6b886e9b931eaaaebb23835b30e217638037b7f2721d4e9c" ;;
    v26.10.21:Xray-linux-64.zip) echo "0eec3ed4163c273d8c8bf821f7d219e257921cf584aa3fb970715e458e631978" ;;
    *) return 1 ;;
  esac
}

release_base="${XRAY_RELEASE_BASE:-https://github.com/XTLS/Xray-core/releases/download/${version}}"
official_release_base="https://github.com/XTLS/Xray-core/releases/download/${version}"

resolve_expected_sha256() {
  if expected_sha256; then
    return 0
  fi
  local dgst_url="${official_release_base}/${asset}.dgst"
  local line
  line="$(curl -fL --retry 2 --connect-timeout 8 --max-time 30 "$dgst_url" | awk '/^SHA2-256=/ {print $2; exit}')"
  [[ -n "$line" ]] || { echo "failed to read SHA2-256 from ${dgst_url}" >&2; return 1; }
  echo "$line"
}

mkdir -p resources/xray
tmp="$(mktemp -t myproxy-xray.XXXXXX.zip)"
trap 'rm -f "$tmp"' EXIT

expected="$(resolve_expected_sha256)" || exit 1
url="${release_base}/${asset}"
if ! curl -fL --retry 2 --connect-timeout 8 --max-time 120 "$url" -o "$tmp"; then
  echo "failed to download Xray ${version} from ${url}" >&2
  exit 1
fi
actual="$(shasum -a 256 "$tmp" | awk '{print $1}')"
if [[ "$actual" != "$expected" ]]; then
  echo "Xray ${version} ${asset}: SHA-256 mismatch (expected ${expected}, got ${actual})" >&2
  exit 1
fi
unzip -p "$tmp" xray > resources/xray/xray
chmod +x resources/xray/xray
echo "installed Xray ${version} → resources/xray/xray"
resources/xray/xray version || true
