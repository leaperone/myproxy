# myproxy

GPUI controller for a bundled mihomo core. Canonical rules: `~/project/vibe/AGENTS.md`.

Strategy JSON under `~/Library/Application Support/myproxy/` is authoritative. Groups are source + name-contains (OR, `*` `?` wildcards) + pins − exact exclude; `name_excludes` drops automatic matches only. Empty condition groups stay empty. Schema 4 ensures a select group named **Telegram** (same sources as `default`/`PROXY`) and retargets the Telegram rule_set `via` off the catch-all. PROCESS-NAME / DOMAIN-KEYWORD / IP-CIDR (Telegram DC ranges) only apply after traffic enters Mixed or TUN. Mixed is HTTP+SOCKS; **系统接管 (TUN)** on **设置** compiles mihomo `tun` (system stack, auto-route) plus a small DNS block. First TUN connect prompts for admin to `chown`/`chmod u+s` the bundled mihomo — not a Network Extension. TUN defaults off. MATCH stays the first group (`default` / `PROXY`). The **节点组** page lists cards; click a card or「添加节点组」opens a modal editor (live preview, source pills, chips, pin/exclude). The **规则** page lists match/via in a table: click a row to edit in the composer (进程/域名/后缀/关键字/网段), right-click for move/via/delete. The **连接** page polls the local mihomo controller while visible (process, destination, chain, bytes, live up/down); empty when the core is disconnected. macOS keeps a menu-bar extra (left-click opens the window, right-click for connect/apply/quit). Developer mode and launch toggles (开机默认启动 / 静默启动 / 轻量模式 / 启动时默认连接) live on **设置**. Lite mode skips the main window until the menu-bar extra opens it. Levels: error / warn / info always go to `~/Library/Application Support/myproxy/myproxy.log`; debug / trace only with developer mode or `MYPROXY_DEV=1`. Lines are `HH:MM:SSZ level target message`. Subscription URLs are not logged. `myproxyctl log` prints the tail. `myproxyctl tun on|off` writes the TUN flag.

Public repo: https://github.com/leaperone/myproxy. Stable Sparkle channel: `https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml`. v0.0.1 is a full zip; later tags run `generate_appcast` against previous zips for deltas.

```sh
scripts/fetch-mihomo.sh
scripts/fetch-sparkle.sh
scripts/dev.sh                 # local watchexec restart + push debug binaries to macbook-air
scripts/install-macbook-air.sh # release .app on macbook-air, mixed-port 7891
scripts/dev-air.sh             # compile here, copy binaries, relaunch Air
scripts/package-macos-app.sh
scripts/release-macos.sh
cargo run --bin myproxyctl -- capabilities
```
