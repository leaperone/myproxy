#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -d resources/sparkle/Sparkle.framework ]]; then
  scripts/fetch-sparkle.sh
fi
if [[ ! -x resources/mihomo/mihomo ]]; then
  scripts/fetch-mihomo.sh
fi
cargo build --release --bins --features sparkle
app="target/release/myproxy.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" "$app/Contents/Frameworks"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
cp target/release/myproxy "$app/Contents/MacOS/myproxy"
cp target/release/myproxyctl "$app/Contents/MacOS/myproxyctl"
cp resources/mihomo/mihomo "$app/Contents/MacOS/mihomo"
cp -R resources/sparkle/Sparkle.framework "$app/Contents/Frameworks/Sparkle.framework"
chmod +x "$app/Contents/MacOS/"*

identity="${CODESIGN_IDENTITY:-}"
if [[ -z "$identity" && "${CODESIGN_ADHOC:-}" != "1" ]]; then
  identity=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application:/ {print $2; exit}')
fi
if [[ -n "${identity:-}" && "$identity" != "-" ]]; then
  codesign --force --deep --options runtime --timestamp --sign "$identity" "$app"
else
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
    echo "CI release requires CODESIGN_IDENTITY (Developer ID); refusing ad-hoc" >&2
    exit 1
  fi
  codesign --force --deep --sign - "$app"
fi
echo "built $app"
