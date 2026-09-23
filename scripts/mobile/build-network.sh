#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
root_dir="$PWD"
if [[ "${CI:-}" != true ]]; then
  echo 'Build native dependencies on GitHub Actions.' >&2
  exit 1
fi
platform="${1:?android or ios}"
go install golang.org/x/mobile/cmd/gomobile@v0.0.0-20260908204917-8b95e45f8d3e
go install golang.org/x/mobile/cmd/gobind@v0.0.0-20260908204917-8b95e45f8d3e
export PATH="$(go env GOPATH)/bin:$PATH"
cd mobile/network
go mod tidy
case "$platform" in
  android)
    sdkmanager 'ndk;27.1.12297006'
    export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.1.12297006"
    mkdir -p "$root_dir/mobile/app/modules/myproxy/android/libs"
    gomobile bind -target=android/arm64 -androidapi=24 -javapkg=one.leaper.myproxy.network \
      -o "$root_dir/mobile/app/modules/myproxy/android/libs/myproxy-network.aar" .
    ;;
  ios)
    mkdir -p "$root_dir/mobile/app/modules/myproxy/ios/Frameworks"
    gomobile bind -target=ios/arm64,iossimulator/arm64 -iosversion=16.4 \
      -o "$root_dir/mobile/app/modules/myproxy/ios/Frameworks/MyProxyNetwork.xcframework" .
    ;;
  *) echo 'Unknown platform' >&2; exit 1 ;;
esac
