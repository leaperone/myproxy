#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
host="${SSH_HOST:-macbook-air}"
port="${AIR_MIXED_PORT:-7891}"
scripts/package-macos-app.sh
ssh "$host" 'mkdir -p ~/Applications ~/.cargo/bin ~/Library/Application\ Support/myproxy
bin="$HOME/.cargo/bin/myproxyctl"
if [[ -x "$bin" ]]; then "$bin" disconnect >/dev/null || true; fi
osascript -e "quit app \"myproxy\"" >/dev/null 2>&1 || true
pkill -x myproxy >/dev/null 2>&1 || true
pkill -f "$HOME/Applications/myproxy.app/Contents/MacOS/mihomo" >/dev/null 2>&1 || true
sleep 0.4'
tar -C target/release -cf - myproxy.app | ssh "$host" 'rm -rf ~/Applications/myproxy.app && tar -C ~/Applications -xf -'
if ! ssh "$host" 'test -f "$HOME/Library/Application Support/myproxy/strategy.json"'; then
  if [[ -f "$HOME/Library/Application Support/myproxy/strategy.json" ]]; then
    scp -q "$HOME/Library/Application Support/myproxy/strategy.json" \
      "$host:Library/Application Support/myproxy/strategy.json"
  fi
fi
if ! ssh "$host" 'test -f "$HOME/Library/Application Support/myproxy/catalog.json"'; then
  if [[ -f "$HOME/Library/Application Support/myproxy/catalog.json" ]]; then
    scp -q "$HOME/Library/Application Support/myproxy/catalog.json" \
      "$host:Library/Application Support/myproxy/catalog.json"
  fi
fi
ssh "$host" "ln -sfn ~/Applications/myproxy.app/Contents/MacOS/myproxyctl ~/.cargo/bin/myproxyctl
xattr -cr ~/Applications/myproxy.app || true
~/.cargo/bin/myproxyctl port ${port}
~/.cargo/bin/myproxyctl apply
open ~/Applications/myproxy.app"
echo "installed ~/Applications/myproxy.app on $host (mixed-port ${port})"
