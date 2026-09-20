---
name: agent
description: Use the installed myproxyctl CLI to inspect and configure myproxy from an Agent or other automation.
---

# Agent CLI for myproxy

Use the bundled `myproxyctl` shipped inside the `.app` (`Contents/MacOS/myproxyctl`) or the `~/.cargo/bin/myproxyctl` link. Do not edit `~/Library/Application Support/myproxy/strategy.json` directly. The release app updates the CLI with the GUI. **设置 → 命令行工具** creates the PATH link; the MacBook Air install script does the same. If the command is missing from `PATH`, add `~/.cargo/bin`.

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

The Xray release channel uses a separate build, data directory, and
`myproxy-xrayctl` link. `backend` reports the compiled backend; it cannot
switch a running process to Xray. Install the Xray channel build to use that backend. Both HTTP and SOCKS5 use the same loopback
port, default 40808. Use `global <node-or-group>` to choose the whole proxy
exit, `group select <group> <node>` to fix a group member, and `group auto
<group>` to restore automatic selection in the Xray build.

Use `subscription list`, `group list`, and `rule list` with `--json` when reading existing configuration. Use `port <number>`, `tun on|off`, and `extension on|off` for transport settings; enabling `extension` disables TUN. Use `mixed-mode rule|proxy|global|direct` and `extension-mode rule|proxy|global|direct` for the two independent inbound routing modes (default `rule`). Use `global [name]` to read or switch mihomo's built-in GLOBAL selector (persisted as `global_selected`; apply/connect restores it). Use `routing allowlist|gfwlist|group|chinadirect` for the rules-page fallback (GFWList is a whole-set Loyalsoldier provider, not flattened rules; chinadirect is GEOIP CN then the default group). `unmatched direct` / `unmatched <group>` remain compat aliases. Use `export [path]` and `import <path>` for the whole `strategy.json` (subscriptions, groups, rules, and machine flags). Omit the export path to write `~/Downloads/myproxy-strategy-YYYY-MM-DD.json`. Import writes `strategy.json.bak-import-*` and does not apply. Never print subscription URLs or logs to a public response unless the operator asks for them.

`--json` is for machine-readable results, not a permission bypass. Keep mutations explicit, check the returned status, and run `apply` after configuration changes that should reach mihomo.

The Xray channel reuses the signed System Extension. Inspect capture and DNS readiness separately from core readiness. The source of an explicit HTTP/SOCKS request is not a trusted process identity. Xray accepts `wildcard`, `network`, `geo-site`, and `geo-ip` matcher kinds through strategy import; network conditions qualify the other matchers. Geographic/domain categories require their local data snapshot. Use the Xray update feed only for this channel.
