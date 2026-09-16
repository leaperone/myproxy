#!/usr/bin/env bash
# Submit a Developer ID .app to notarytool, require Accepted, then staple.
# Does not print APPLE_* values.
set -euo pipefail

app="${1:-target/release/myproxy.app}"
result="${2:-}"

if [[ ! -d "$app" ]]; then
  echo "missing app bundle: $app" >&2
  exit 1
fi
for name in APPLE_ID APPLE_APP_SPECIFIC_PASSWORD APPLE_TEAM_ID; do
  if [[ -z "${!name:-}" ]]; then
    echo "missing ${name}" >&2
    exit 1
  fi
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
zip_path="$tmp/myproxy-notarize.zip"
if [[ -z "$result" ]]; then
  result="$tmp/notarization.json"
fi
mkdir -p "$(dirname "$result")"

ditto -c -k --keepParent "$app" "$zip_path"
set +x
notary_exit=0
xcrun notarytool submit "$zip_path" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_APP_SPECIFIC_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait --output-format json > "$result" || notary_exit=$?
cat "$result"
notary_status=$(plutil -extract status raw -o - "$result" 2>/dev/null) || notary_status=""
if [[ "$notary_exit" -ne 0 || "$notary_status" != "Accepted" ]]; then
  if submission_id=$(plutil -extract id raw -o - "$result" 2>/dev/null); then
    xcrun notarytool log "$submission_id" \
      --apple-id "$APPLE_ID" \
      --password "$APPLE_APP_SPECIFIC_PASSWORD" \
      --team-id "$APPLE_TEAM_ID" >&2 || true
  fi
  echo "Notarization was not accepted (status: ${notary_status:-unavailable}, exit: $notary_exit)" >&2
  exit 1
fi
set -euo pipefail
xcrun stapler staple "$app"
