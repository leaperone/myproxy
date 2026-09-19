#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Build a disposable Xray-channel package without touching dist/, appcast, or
# any stable/nightly release asset. The normal package script is reused only
# to produce the host application; this script copies Xray into a new bundle.
scripts/fetch-xray.sh
if [[ ! -d target/release/myproxy.app ]]; then
  # The regular script is kept byte-for-byte out of the Xray test diff. Its
  # ad-hoc path uses an empty Bash array, so create a disposable copy with the
  # nounset-safe expansion needed on the CI shell.
  patched_script="scripts/.package-macos-app-xray-test.sh"
  trap 'rm -f "$patched_script"' EXIT
  sed 's/"${extra\[@\]}"/${extra[@]+"${extra[@]}"}/g' \
    scripts/package-macos-app.sh > "$patched_script"
  chmod +x "$patched_script"
  GITHUB_ACTIONS= CODESIGN_ADHOC=1 MYPROXY_BUILD_CHANNEL=dev "$patched_script"
fi

version="${MYPROXY_XRAY_TEST_VERSION:-$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)}"
out="target/xray-channel/myproxy-xray-channel.app"
rm -rf "$(dirname "$out")"
mkdir -p "$(dirname "$out")"
cp -R target/release/myproxy.app "$out"
cp resources/xray/xray "$out/Contents/MacOS/xray"
chmod +x "$out/Contents/MacOS/xray"

# Adding a binary invalidates the copied app signature. This is a disposable
# local test artifact; release signing remains owned solely by the existing
# Prod/Nightly workflow. Open it with the normal macOS right-click flow.
mkdir -p dist/xray-channel
archive="dist/xray-channel/myproxy-xray-${version}.zip"
rm -f "$archive"
ditto -c -k --keepParent "$out" "$archive"
echo "wrote disposable Xray test package $archive"
