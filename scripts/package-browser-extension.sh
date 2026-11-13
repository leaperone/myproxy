#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

browser="${1:-}"
case "$browser" in
  chromium|firefox|safari) ;;
  *)
    echo "usage: $0 chromium|firefox|safari" >&2
    exit 2
    ;;
esac

source_dir="browser-extension"
output_dir="target/browser-extension/$browser"
mkdir -p "$output_dir"

cp "$source_dir/background.js" "$source_dir/popup.html" "$source_dir/popup.css" "$source_dir/popup.js" "$output_dir/"

case "$browser" in
  chromium) cp "$source_dir/manifest.chromium.json" "$output_dir/manifest.json" ;;
  firefox) cp "$source_dir/manifest.firefox.json" "$output_dir/manifest.json" ;;
  safari) cp "$source_dir/manifest.safari.json" "$output_dir/manifest.json" ;;
esac

echo "packaged $output_dir"
