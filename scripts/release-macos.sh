#!/usr/bin/env bash
# Package myproxy.app, zip it for Sparkle, and refresh appcast.xml.
# Prod retains previous stable zips for deltas; Nightly publishes full archives.
set -euo pipefail
cd "$(dirname "$0")/.."

channel="${MYPROXY_BUILD_CHANNEL:-prod}"
version="${1:-}"
if [[ -z "$version" ]]; then
  version=$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)
fi
version="${version#v}"
tag="${MYPROXY_RELEASE_TAG:-v${version}}"
build_number="${MYPROXY_BUILD_NUMBER:-$version}"
if [[ ! "$channel" =~ ^(prod|nightly)$ || ! "$version" =~ ^[0-9][0-9A-Za-z.+-]*$ || ! "$tag" =~ ^[a-zA-Z0-9][a-zA-Z0-9._+-]*$ || ! "$build_number" =~ ^[0-9]+(\.[0-9]+)*$ ]]; then
  echo "invalid release channel, version, tag, or numeric MYPROXY_BUILD_NUMBER" >&2
  exit 1
fi
export MYPROXY_BUILD_CHANNEL="$channel"
export MYPROXY_VERSION="$version"
export MYPROXY_BUILD_NUMBER="$build_number"

scripts/fetch-mihomo.sh
scripts/fetch-sparkle.sh
scripts/package-macos-app.sh

app="target/release/myproxy.app"
dist="dist"
archives="${dist}/sparkle-archives"
rm -rf "$dist"
mkdir -p "$archives"

zip_name="myproxy-${version}.sparkle.zip"
zip_path="${dist}/${zip_name}"

notarize=0
if [[ "${SKIP_NOTARIZE:-}" != "1" && -n "${APPLE_ID:-}" && -n "${APPLE_APP_SPECIFIC_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
  identity=$(security find-identity -v -p codesigning 2>/dev/null \
    | awk -F'"' '/Developer ID Application:/ {print $2; exit}')
  if [[ -n "${identity:-}" ]]; then
    notarize=1
  fi
fi

if [[ "$notarize" -eq 1 ]]; then
  echo "submitting app for notarization"
  ditto -c -k --keepParent "$app" "$zip_path"
  set +x
  xcrun notarytool submit "$zip_path" \
    --apple-id "$APPLE_ID" \
    --password "$APPLE_APP_SPECIFIC_PASSWORD" \
    --team-id "$APPLE_TEAM_ID" \
    --wait
  set -euo pipefail
  xcrun stapler staple "$app"
  rm -f "$zip_path"
elif [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  echo "CI release requires notarization (APPLE_ID, APPLE_APP_SPECIFIC_PASSWORD, APPLE_TEAM_ID, and a Developer ID identity)" >&2
  exit 1
fi

ditto -c -k --keepParent "$app" "$zip_path"

# Prod deltas only use published Prod archives; Nightly ships full archives.
# ponytail: add Nightly deltas when daily download size warrants retaining a cache.
if [[ "$channel" == "prod" ]] && command -v gh >/dev/null && gh repo view >/dev/null 2>&1; then
  while read -r old_tag; do
    [[ -n "$old_tag" && "$old_tag" != "$tag" ]] || continue
    gh release download "$old_tag" --pattern 'myproxy-*.sparkle.zip' --dir "$archives" --clobber 2>/dev/null || true
  done < <(gh release list --exclude-drafts --exclude-pre-releases --limit 6 --json tagName --jq '.[].tagName' 2>/dev/null || true)
fi

cp "$zip_path" "$archives/$zip_name"
printf 'myproxy %s (%s)\n' "$version" "$channel" \
  > "$archives/myproxy-${version}.sparkle.md"

appcast_args=(
  --download-url-prefix "https://github.com/leaperone/myproxy/releases/download/${tag}/"
  --link "https://github.com/leaperone/myproxy/releases/tag/${tag}"
  --embed-release-notes
  --maximum-deltas 5
  --maximum-versions 1
  --versions "$build_number"
  -o "$archives/appcast.xml"
)
if [[ "$channel" == "nightly" ]]; then
  appcast_args+=(--channel nightly)
fi

if [[ -n "${SPARKLE_ED_PRIVATE_KEY:-}" ]]; then
  set +x
  printf '%s\n' "$SPARKLE_ED_PRIVATE_KEY" | ./resources/sparkle/bin/generate_appcast \
    --ed-key-file - \
    "${appcast_args[@]}" \
    "$archives"
  set -euo pipefail
elif [[ -f "${SPARKLE_ED_KEY_FILE:-.secrets/sparkle_eddsa}" ]]; then
  ./resources/sparkle/bin/generate_appcast \
    --ed-key-file "${SPARKLE_ED_KEY_FILE:-.secrets/sparkle_eddsa}" \
    "${appcast_args[@]}" \
    "$archives"
else
  echo "no Sparkle EdDSA private key; writing unsigned enclosure list" >&2
  length=$(wc -c < "$zip_path" | tr -d ' ')
  channel_element=""
  if [[ "$channel" == "nightly" ]]; then
    channel_element='<sparkle:channel>nightly</sparkle:channel>'
  fi
  cat > "$archives/appcast.xml" <<XML
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>myproxy</title>
    <item>
      ${channel_element}
      <title>Version ${version}</title>
      <pubDate>$(date -u '+%a, %d %b %Y %H:%M:%S +0000')</pubDate>
      <sparkle:version>${build_number}</sparkle:version>
      <sparkle:shortVersionString>${version}</sparkle:shortVersionString>
      <enclosure url="https://github.com/leaperone/myproxy/releases/download/${tag}/${zip_name}" length="${length}" type="application/octet-stream"/>
    </item>
  </channel>
</rss>
XML
fi

cp "$archives/appcast.xml" "$dist/appcast.xml"
# Deltas land next to the zips when a previous archive exists.
shopt -s nullglob
for delta in "$archives"/*.delta; do
  cp "$delta" "$dist/"
done
echo "release artifacts in $dist"
ls -la "$dist"
