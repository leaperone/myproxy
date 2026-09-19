#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

version="${MYPROXY_XRAY_TEST_VERSION:-1.6.0-xray-test}"
export MYPROXY_BUILD_CHANNEL=dev
export MYPROXY_VERSION="$version"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target/xray-build}"
./scripts/fetch-xray.sh
cargo build --locked --release --bins --features xray-channel

stage="$(mktemp -d "${TMPDIR:-/tmp}/myproxy-xray-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
app="$stage/MyProxy Xray.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$CARGO_TARGET_DIR/release/myproxy" "$app/Contents/MacOS/myproxy"
cp "$CARGO_TARGET_DIR/release/myproxyctl" "$app/Contents/MacOS/myproxyctl"
cp resources/xray/xray "$app/Contents/MacOS/xray"
cp packaging/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
python3 - "$app" "$version" <<'PYINFO'
import plistlib, sys
from pathlib import Path
app, version = Path(sys.argv[1]), sys.argv[2]
info = {
    'CFBundleIdentifier': 'one.leaper.myproxy.xray-test',
    'CFBundleName': 'MyProxy Xray', 'CFBundleDisplayName': 'MyProxy Xray',
    'CFBundleExecutable': 'myproxy', 'CFBundlePackageType': 'APPL',
    'CFBundleShortVersionString': version, 'CFBundleVersion': '1',
    'CFBundleIconFile': 'AppIcon', 'NSHighResolutionCapable': True,
    'LSMinimumSystemVersion': '14.0', 'MyproxyBuildChannel': 'xray-test',
}
(app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
PYINFO
for executable in xray myproxyctl myproxy; do
    chmod +x "$app/Contents/MacOS/$executable"
    codesign --force --sign - "$app/Contents/MacOS/$executable"
done
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
mkdir -p dist/xray-channel
archive="dist/xray-channel/myproxy-xray-${version}.zip"
test ! -e "$archive" || { echo "artifact already exists: $archive" >&2; exit 1; }
ditto -c -k --keepParent "$app" "$archive"
python3 scripts/check-xray-package.py "$archive"
shasum -a 256 "$archive" > "$archive.sha256"
echo "Xray test artifact: $archive"
