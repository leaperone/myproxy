---
name: agent
description: Use the installed myproxyctl CLI to inspect and configure myproxy from an Agent or other automation.
---

# Agent CLI for myproxy

Use the bundled `myproxyctl` shipped inside the `.app` (`Contents/MacOS/myproxyctl`) or the `~/.cargo/bin/myproxyctl` link. Do not edit `~/Library/Application Support/myproxy/strategy.json` directly. The release app updates the CLI with the GUI. A first-launch dialog or **设置 → 命令行工具** creates the PATH link; the MacBook Air install script does the same. If the command is missing from `PATH`, add `~/.cargo/bin`.

Put `--json` before or after the command. Successful commands emit one JSON value on stdout. Errors use stderr and a non-zero exit; do not parse an error as a successful JSON result. Discover the current command surface with:

```sh
myproxyctl --json capabilities
myproxyctl --json status
```

Common configuration flow:

```sh
myproxyctl --json subscription add 'https://…' --name Example
myproxyctl --json group add PROXY --all
myproxyctl --json rule add --name GitHub --keyword github --via PROXY
myproxyctl --json apply
myproxyctl --json connect
```

Use `subscription list`, `group list`, and `rule list` with `--json` when reading existing configuration. Use `port <number>`, `tun on|off`, and `extension on|off` for transport settings; enabling `extension` disables TUN. Never print subscription URLs or logs to a public response unless the operator asks for them.

`--json` is for machine-readable results, not a permission bypass. Keep mutations explicit, check the returned status, and run `apply` after configuration changes that should reach mihomo.
