#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
root_dir="$PWD"
if [[ "${CI:-}" != true ]]; then
  echo 'Mobile builds run in GitHub Actions; no local SDK or dependency installation.' >&2
  exit 1
fi
node scripts/mobile/prepare-version.js
ndk_version=27.1.12297006
sdkmanager "ndk;$ndk_version"
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/$ndk_version"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android24-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS='-C link-arg=-Wl,-z,max-page-size=16384'
cargo build -p myproxy-mobile --release --target aarch64-linux-android
mkdir -p mobile/app/modules/myproxy/android/src/main/jniLibs/arm64-v8a mobile/app/modules/myproxy/android/libs
cp target/aarch64-linux-android/release/libmyproxy_mobile.so mobile/app/modules/myproxy/android/src/main/jniLibs/arm64-v8a/
if [[ ! -f mobile/app/modules/myproxy/android/libs/myproxy-network.aar ]]; then
  bash scripts/mobile/build-network.sh android
fi
cd "$root_dir/mobile/app"
npm ci --no-audit --no-fund
npx expo prebuild --platform android --no-install
node "$root_dir/scripts/mobile/verify-autolinking.js" android
cd android
./gradlew assembleRelease -PreactNativeArchitectures=arm64-v8a -Pandroid.minSdkVersion=26 --build-cache --no-daemon
python3 "$root_dir/scripts/mobile/verify-package.py" android app/build/outputs/apk/release/app-release.apk \
  --report app/build/outputs/apk/release/build-manifest.json
