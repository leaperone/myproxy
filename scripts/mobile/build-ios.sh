#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
root_dir="$PWD"
if [[ "${CI:-}" != true ]]; then
  echo 'Mobile builds run in GitHub Actions; no local SDK or dependency installation.' >&2
  exit 1
fi
framework_dir="$root_dir/mobile/app/modules/myproxy/ios/Frameworks"
mkdir -p "$framework_dir" mobile/.build/artifacts mobile/.build/logs
cargo build -p myproxy-mobile --release --target aarch64-apple-ios
cargo build -p myproxy-mobile --release --target aarch64-apple-ios-sim
xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libmyproxy_mobile.a -headers crates/myproxy-mobile/include \
  -library target/aarch64-apple-ios-sim/release/libmyproxy_mobile.a -headers crates/myproxy-mobile/include \
  -output "$framework_dir/MyProxyCore.xcframework"
go install golang.org/x/mobile/cmd/gomobile@v0.0.0-20260908204917-8b95e45f8d3e
go install golang.org/x/mobile/cmd/gobind@v0.0.0-20260908204917-8b95e45f8d3e
export PATH="$(go env GOPATH)/bin:$PATH"
cd mobile/network
go mod tidy
gomobile bind -target=ios/arm64,iossimulator/arm64 -iosversion=16.4 -o "$framework_dir/MyProxyNetwork.xcframework" .
cd "$root_dir/mobile/app"
npm install --no-audit --no-fund
npx expo prebuild --platform ios --no-install
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
