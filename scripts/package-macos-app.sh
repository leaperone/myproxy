#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --bins
if [[ ! -x resources/mihomo/mihomo ]]; then
  scripts/fetch-mihomo.sh
fi
app="target/release/myproxy.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
cp target/release/myproxy "$app/Contents/MacOS/myproxy"
cp target/release/myproxyctl "$app/Contents/MacOS/myproxyctl"
cp resources/mihomo/mihomo "$app/Contents/MacOS/mihomo"
chmod +x "$app/Contents/MacOS/"*
codesign --force --deep --sign - "$app" >/dev/null
echo "built $app"
