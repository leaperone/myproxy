#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

version="${MYPROXY_XRAY_VERSION:-1.6.0-xray.2}"
build_number="${MYPROXY_XRAY_BUILD_NUMBER:-1.6.0.1}"
target_dir="${CARGO_TARGET_DIR:-target/xray-build}"
identity="${CODESIGN_IDENTITY:-}"
host_profile="${MYPROXY_HOST_DEVID_PROFILE_PATH:-}"
extension_profile="${MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE_PATH:-}"
[[ "$version" =~ ^[0-9][0-9A-Za-z.+-]*$ ]] || { echo "invalid Xray version" >&2; exit 1; }
[[ "$build_number" =~ ^[0-9]+(\.[0-9]+)*$ ]] || { echo "invalid Xray build number" >&2; exit 1; }
[[ -n "$identity" ]] || { echo "CODESIGN_IDENTITY is required for a distributable Xray build" >&2; exit 1; }
[[ -s "$host_profile" && -s "$extension_profile" ]] || { echo "Developer ID provisioning profiles are required" >&2; exit 1; }

export MYPROXY_BUILD_CHANNEL=xray MYPROXY_VERSION="$version" CARGO_TARGET_DIR="$target_dir"
scripts/fetch-xray.sh
scripts/fetch-sparkle.sh
MYPROXY_XRAY_CHANNEL=1 scripts/build-network-extension.sh target/xray-network-extension
python3 scripts/check-xray-admission-client.py target/xray-network-extension
python3 scripts/test-xray-dns-tcp-framing.py
cargo build --locked --release --bins --features 'sparkle,xray-channel'

stage="$(mktemp -d "${TMPDIR:-/tmp}/myproxy-xray-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
app="$stage/MyProxy.app"
extension_id="local.harry.myproxy.network-extension"
system_extension="$app/Contents/Library/SystemExtensions/${extension_id}.systemextension"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/zh-Hans.lproj" "$app/Contents/Frameworks" "$system_extension/Contents/MacOS"
mkdir -p "$app/Contents/Resources/ThirdParty/xray"
cp ThirdParty/xray/NOTICE.md ThirdParty/xray/LICENSE "$app/Contents/Resources/ThirdParty/xray/"
cp packaging/macos/Info.plist "$app/Contents/Info.plist"
cp packaging/macos/AppIcon.icns "$app/Contents/Resources/AppIcon.icns"
cp packaging/macos/zh-Hans.lproj/InfoPlist.strings "$app/Contents/Resources/zh-Hans.lproj/InfoPlist.strings"
cp "$target_dir/release/myproxy" "$app/Contents/MacOS/myproxy"
cp "$target_dir/release/myproxyctl" "$app/Contents/MacOS/myproxyctl"
cp resources/xray/xray "$app/Contents/MacOS/xray"
cp -R resources/sparkle/Sparkle.framework "$app/Contents/Frameworks/Sparkle.framework"
cp packaging/macos/NetworkExtension/Info.plist "$system_extension/Contents/Info.plist"
cp target/xray-network-extension/MyproxyNetworkExtension "$system_extension/Contents/MacOS/MyproxyNetworkExtension"
chmod +x "$app/Contents/MacOS/"* "$system_extension/Contents/MacOS/MyproxyNetworkExtension"

python3 - "$app/Contents/Info.plist" "$system_extension/Contents/Info.plist" "$version" "$build_number" <<'PY'
import plistlib, sys
from pathlib import Path
host, extension, version, build = map(Path, sys.argv[1:])
for path in (host, extension):
    info = plistlib.loads(path.read_bytes())
    info['CFBundleShortVersionString'] = str(version)
    info['CFBundleVersion'] = str(build)
    if path == host:
        info['CFBundleIdentifier'] = 'local.harry.myproxy'
        info['CFBundleName'] = 'myproxy'
        info['CFBundleDisplayName'] = 'MyProxy'
        info['MyproxyBuildChannel'] = 'xray'
        info['SUFeedURL'] = 'https://github.com/leaperone/myproxy/releases/download/xray/appcast.xml'
    else:
        info['CFBundleIdentifier'] = 'local.harry.myproxy.network-extension'
    path.write_bytes(plistlib.dumps(info, sort_keys=False))
PY
cp "$host_profile" "$app/Contents/embedded.provisionprofile"
cp "$extension_profile" "$system_extension/Contents/embedded.provisionprofile"
chmod 600 "$app/Contents/embedded.provisionprofile" "$system_extension/Contents/embedded.provisionprofile"

sign_nested() {
    codesign --force --options runtime --timestamp --identifier local.harry.myproxy.xray --sign "$identity" "$app/Contents/MacOS/xray"
    codesign --force --options runtime --timestamp --sign "$identity" "$app/Contents/MacOS/myproxyctl"
    local sparkle="$app/Contents/Frameworks/Sparkle.framework/Versions/B"
    for nested in "$sparkle/XPCServices/Installer.xpc" "$sparkle/XPCServices/Downloader.xpc" "$sparkle/Autoupdate" "$sparkle/Updater.app"; do
        if [[ -e "$nested" ]]; then
            if [[ "$nested" == */Downloader.xpc ]]; then
                codesign --force --options runtime --timestamp --preserve-metadata=entitlements --sign "$identity" "$nested"
            else
                codesign --force --options runtime --timestamp --sign "$identity" "$nested"
            fi
        fi
    done
    codesign --force --options runtime --timestamp --sign "$identity" "$app/Contents/Frameworks/Sparkle.framework"
}
sign_nested
codesign --force --options runtime --timestamp --entitlements packaging/macos/NetworkExtension/DeveloperID.entitlements --sign "$identity" "$system_extension"
codesign --force --options runtime --timestamp --entitlements packaging/macos/Signing/Host-DeveloperID.entitlements --sign "$identity" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

dist="dist/xray-channel"
rm -rf "$dist"
mkdir -p "$dist/sparkle-archives"
for name in APPLE_ID APPLE_APP_SPECIFIC_PASSWORD APPLE_TEAM_ID SPARKLE_ED_PRIVATE_KEY MYPROXY_RELEASE_TAG; do
    [[ -n "${!name:-}" ]] || { echo "${name} is required" >&2; exit 1; }
done
scripts/notarize-macos-app.sh "$app" "$dist/notarization.json"
xcrun stapler validate "$app"
spctl --assess --type execute --verbose=2 "$app"
archive="$dist/myproxy-xray-${version}.sparkle.zip"
ditto -c -k --keepParent "$app" "$archive"
cp "$archive" "$dist/sparkle-archives/"
printf '%s\n' "$SPARKLE_ED_PRIVATE_KEY" | resources/sparkle/bin/generate_appcast --ed-key-file - \
    --download-url-prefix "https://github.com/leaperone/myproxy/releases/download/${MYPROXY_RELEASE_TAG}/" \
    --link "https://github.com/leaperone/myproxy/releases/tag/${MYPROXY_RELEASE_TAG}" \
    --channel xray --maximum-versions 1 --versions "$build_number" \
    -o "$dist/appcast.xml" "$dist/sparkle-archives"
python3 scripts/check-xray-package.py "$archive" "$dist/appcast.xml"
shasum -a 256 "$archive" > "$archive.sha256"
echo "signed Xray artifacts in $dist"
