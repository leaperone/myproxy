#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
host="${SSH_HOST:-macbook-air}"
scripts/package-macos-app.sh
ssh "$host" 'mkdir -p ~/Applications ~/.cargo/bin ~/Library/Application\ Support/myproxy'
tar -C target/release -cf - myproxy.app | ssh "$host" 'rm -rf ~/Applications/myproxy.app && tar -C ~/Applications -xf -'
if [[ -f "$HOME/Library/Application Support/myproxy/strategy.json" ]]; then
  scp -q "$HOME/Library/Application Support/myproxy/strategy.json" \
    "$host:Library/Application Support/myproxy/strategy.json"
fi
if [[ -f "$HOME/Library/Application Support/myproxy/catalog.json" ]]; then
  scp -q "$HOME/Library/Application Support/myproxy/catalog.json" \
    "$host:Library/Application Support/myproxy/catalog.json"
fi
ssh "$host" 'ln -sfn ~/Applications/myproxy.app/Contents/MacOS/myproxyctl ~/.cargo/bin/myproxyctl
xattr -cr ~/Applications/myproxy.app || true
open ~/Applications/myproxy.app'
echo "installed ~/Applications/myproxy.app on $host"
