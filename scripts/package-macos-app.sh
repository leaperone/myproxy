#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ ! -d resources/sparkle/Sparkle.framework ]]; then
  scripts/fetch-sparkle.sh
fi
if [[ ! -x resources/mihomo/mihomo ]]; then
  scripts/fetch-mihomo.sh
fi

host_profile="${MYPROXY_HOST_DEVID_PROFILE_PATH:-}"
extension_profile="${MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE_PATH:-}"
host_entitlements_devid="packaging/macos/Signing/Host-DeveloperID.entitlements"
host_entitlements_dev="packaging/macos/Signing/Host-Development.entitlements"
extension_entitlements_devid="packaging/macos/NetworkExtension/DeveloperID.entitlements"
extension_entitlements_dev="packaging/macos/NetworkExtension/Development.entitlements"
extension_bundle_id="local.harry.myproxy.network-extension"

scripts/build-network-extension.sh target/network-extension
cargo build --release --bins --features sparkle

app="target/release/myproxy.app"
rm -rf "$app"
mkdir -p \
  "$app/Contents/MacOS" \
  "$app/Contents/Resources/zh-Hans.lproj" \
  "$app/Contents/Frameworks" \
  "$app/Contents/Library/SystemExtensions/${extension_bundle_id}.systemextension/Contents/MacOS"

cp packaging/macos/Info.plist "$app/Contents/Info.plist"
if /usr/libexec/PlistBuddy -c 'Print :NSSystemExtensionUsageDescription' "$app/Contents/Info.plist" >/dev/null 2>&1; then
  /usr/libexec/PlistBuddy -c \
    'Set :NSSystemExtensionUsageDescription myproxy uses a network system extension to intercept outbound connections and apply your process and routing rules.' \
    "$app/Contents/Info.plist"
else
  /usr/libexec/PlistBuddy -c \
    'Add :NSSystemExtensionUsageDescription string myproxy uses a network system extension to intercept outbound connections and apply your process and routing rules.' \
    "$app/Contents/Info.plist"
fi
cp target/release/myproxy "$app/Contents/MacOS/myproxy"
cp target/release/myproxyctl "$app/Contents/MacOS/myproxyctl"
cp resources/mihomo/mihomo "$app/Contents/MacOS/mihomo"
cp -R resources/sparkle/Sparkle.framework "$app/Contents/Frameworks/Sparkle.framework"
cp packaging/macos/zh-Hans.lproj/InfoPlist.strings \
  "$app/Contents/Resources/zh-Hans.lproj/InfoPlist.strings"

system_extension="$app/Contents/Library/SystemExtensions/${extension_bundle_id}.systemextension"
cp packaging/macos/NetworkExtension/Info.plist "$system_extension/Contents/Info.plist"
cp target/network-extension/MyproxyNetworkExtension \
  "$system_extension/Contents/MacOS/MyproxyNetworkExtension"
chmod +x "$app/Contents/MacOS/"* "$system_extension/Contents/MacOS/MyproxyNetworkExtension"

identity="${CODESIGN_IDENTITY:-}"
if [[ -z "$identity" && "${CODESIGN_ADHOC:-}" != "1" ]]; then
  identity=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application:/ {print $2; exit}')
fi

sign_nested() {
  local sign_identity="$1"
  local extra=()
  if [[ "$sign_identity" != "-" ]]; then
    extra+=(--options runtime --timestamp)
  fi
  codesign --force "${extra[@]}" --identifier local.harry.myproxy.mihomo \
    --sign "$sign_identity" "$app/Contents/MacOS/mihomo"
  codesign --force "${extra[@]}" --sign "$sign_identity" "$app/Contents/MacOS/myproxyctl"
  if [[ -d "$app/Contents/Frameworks/Sparkle.framework/Versions/B" ]]; then
    local sparkle="$app/Contents/Frameworks/Sparkle.framework/Versions/B"
    if [[ -d "$sparkle/XPCServices/Installer.xpc" ]]; then
      codesign --force "${extra[@]}" --sign "$sign_identity" "$sparkle/XPCServices/Installer.xpc"
    fi
    if [[ -d "$sparkle/XPCServices/Downloader.xpc" ]]; then
      codesign --force "${extra[@]}" --preserve-metadata=entitlements \
        --sign "$sign_identity" "$sparkle/XPCServices/Downloader.xpc"
    fi
    if [[ -d "$sparkle/Autoupdate" ]]; then
      codesign --force "${extra[@]}" --sign "$sign_identity" "$sparkle/Autoupdate"
    fi
    if [[ -d "$sparkle/Updater.app" ]]; then
      codesign --force "${extra[@]}" --sign "$sign_identity" "$sparkle/Updater.app"
    fi
    codesign --force "${extra[@]}" --sign "$sign_identity" "$app/Contents/Frameworks/Sparkle.framework"
  fi
}

if [[ -n "${identity:-}" && "$identity" != "-" ]]; then
  if [[ -n "$host_profile" && -s "$host_profile" && -n "$extension_profile" && -s "$extension_profile" ]]; then
    cp "$host_profile" "$app/Contents/embedded.provisionprofile"
    cp "$extension_profile" "$system_extension/Contents/embedded.provisionprofile"
    chmod 600 "$app/Contents/embedded.provisionprofile" \
      "$system_extension/Contents/embedded.provisionprofile"
  else
    echo "Developer ID Network Extension profiles are unset (MYPROXY_HOST_DEVID_PROFILE_PATH / MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE_PATH). Embedding the system extension without profiles; activation will fail until they are provided." >&2
  fi
  sign_nested "$identity"
  codesign --force --options runtime --timestamp \
    --entitlements "$extension_entitlements_devid" \
    --sign "$identity" "$system_extension"
  codesign --force --options runtime --timestamp \
    --entitlements "$host_entitlements_devid" \
    --sign "$identity" "$app"
else
  if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
    echo "CI release requires CODESIGN_IDENTITY (Developer ID); refusing ad-hoc" >&2
    exit 1
  fi
  sign_nested "-"
  codesign --force --entitlements "$extension_entitlements_dev" --sign - "$system_extension"
  codesign --force --entitlements "$host_entitlements_dev" --sign - "$app"
fi
echo "built $app"
