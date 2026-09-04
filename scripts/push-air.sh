#!/usr/bin/env bash
# Copy locally built myproxy binaries onto macbook-air and relaunch.
set -euo pipefail
cd "$(dirname "$0")/.."
host="${SSH_HOST:-macbook-air}"
if [[ "${RELEASE:-}" == "1" ]]; then
  profile=release
  cargo build --release --bins
else
  profile=debug
  cargo build --bins
fi
remote_app='~/Applications/myproxy.app'
remote_macos="$remote_app/Contents/MacOS"
if ! ssh "$host" "test -x $remote_macos/myproxy"; then
  echo "no $remote_app on $host; run scripts/install-macbook-air.sh first" >&2
  exit 1
fi
ssh "$host" 'bin="$HOME/.cargo/bin/myproxyctl"
if [[ -x "$bin" ]]; then "$bin" disconnect >/dev/null || true; fi
osascript -e "quit app \"myproxy\"" >/dev/null 2>&1 || true
pkill -x myproxy >/dev/null 2>&1 || true
pkill -f "$HOME/Applications/myproxy.app/Contents/MacOS/mihomo" >/dev/null 2>&1 || true
sleep 0.4'
scp -q "target/$profile/myproxy" "target/$profile/myproxyctl" "$host:Applications/myproxy.app/Contents/MacOS/"
ssh "$host" 'chmod +x ~/Applications/myproxy.app/Contents/MacOS/myproxy ~/Applications/myproxy.app/Contents/MacOS/myproxyctl
ln -sfn ~/Applications/myproxy.app/Contents/MacOS/myproxyctl ~/.cargo/bin/myproxyctl
xattr -cr ~/Applications/myproxy.app || true
codesign --force --sign - ~/Applications/myproxy.app/Contents/MacOS/myproxy ~/Applications/myproxy.app/Contents/MacOS/myproxyctl >/dev/null 2>&1 || true
open ~/Applications/myproxy.app'
echo "pushed $profile binaries to $host"
