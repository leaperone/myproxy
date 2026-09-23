#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
root_dir="$PWD"
if [[ "${CI:-}" != true ]]; then
  echo 'Mobile builds run in GitHub Actions; no local SDK or dependency installation.' >&2
  exit 1
fi
node scripts/mobile/prepare-version.js
framework_dir="$root_dir/mobile/app/modules/myproxy/ios/Frameworks"
mkdir -p "$framework_dir" mobile/.build/artifacts mobile/.build/logs
cargo build -p myproxy-mobile --release --target aarch64-apple-ios
cargo build -p myproxy-mobile --release --target aarch64-apple-ios-sim
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libmyproxy_mobile.a -headers crates/myproxy-mobile/include \
  -library target/aarch64-apple-ios-sim/release/libmyproxy_mobile.a -headers crates/myproxy-mobile/include \
  -output "$framework_dir/MyProxyCore.xcframework"
if [[ ! -d "$framework_dir/MyProxyNetwork.xcframework" ]]; then
  bash scripts/mobile/build-network.sh ios
fi
cd "$root_dir/mobile/app"
npm ci --no-audit --no-fund
npx expo prebuild --platform ios --no-install
node "$root_dir/scripts/mobile/verify-autolinking.js" apple
cd "$root_dir"
ruby mobile/platform/configure-ios.rb
cd mobile/app/ios
pod install
workspace_path="$(find . -maxdepth 1 -name '*.xcworkspace' -print -quit)"
scheme_name="$(basename "$workspace_path" .xcworkspace)"
xcodebuild -workspace "$workspace_path" -scheme "$scheme_name" -configuration Release \
  -sdk iphonesimulator -destination 'generic/platform=iOS Simulator' ARCHS=arm64 \
  CODE_SIGNING_ALLOWED=NO -derivedDataPath "$root_dir/mobile/.build/simulator" \
  build > "$root_dir/mobile/.build/logs/simulator.log" 2>&1 || { tail -100 "$root_dir/mobile/.build/logs/simulator.log"; exit 1; }
xcodebuild -workspace "$workspace_path" -scheme "$scheme_name" -configuration Release \
  -sdk iphoneos -destination 'generic/platform=iOS' ARCHS=arm64 \
  CODE_SIGNING_ALLOWED=NO -archivePath "$root_dir/mobile/.build/MyProxy.xcarchive" \
  archive > "$root_dir/mobile/.build/logs/device.log" 2>&1 || { tail -100 "$root_dir/mobile/.build/logs/device.log"; exit 1; }
ditto -c -k --keepParent "$root_dir/mobile/.build/simulator/Build/Products/Release-iphonesimulator/$scheme_name.app" "$root_dir/mobile/.build/artifacts/MyProxy-iOS-Simulator.zip"
ditto -c -k --keepParent "$root_dir/mobile/.build/MyProxy.xcarchive" "$root_dir/mobile/.build/artifacts/MyProxy-iOS-Unsigned.xcarchive.zip"
