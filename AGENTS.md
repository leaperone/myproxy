# myproxy

GPUI controller for a bundled mihomo core. Canonical rules: `~/project/vibe/AGENTS.md`.

Strategy JSON under `~/Library/Application Support/myproxy/` is authoritative. Groups are source + name-contains (OR, `*` `?` wildcards) + pins − exact exclude; `name_excludes` drops automatic matches only. Empty condition groups stay empty. Schema 5 ensures a select group named **Telegram** (same sources as `default`/`PROXY`) and retargets a catch-all Telegram rule_set `via` to that group. PROCESS-NAME, DOMAIN-SUFFIX (`telegram.org` / `t.me` / …), and IP-CIDR (published DC ranges) only apply after traffic enters Mixed, System Extension, or TUN. Domain rules match HTTP(S) SNI/host (sniffer when TUN is on), not raw MTProto to DC IPs. Mixed is HTTP+SOCKS and stays available for explicit clients. **系统接管** on **设置** is MClash-style `NETransparentProxyProvider` (System Extension): process/`app` matchers compile into NE capture rules that pin a private loopback SOCKS listener (`proxy:` = via group); remaining traffic uses a profile-rules SOCKS inbound so domain/IP/MATCH still hit the same mihomo strategy. First enable asks the user to allow myproxy in **系统设置 › 通用 › 登录项与扩展**. Needs a Developer ID `.app`; release signing can embed `MYPROXY_HOST_DEVID_PROFILE` / `MYPROXY_NETWORK_EXTENSION_DEVID_PROFILE` (same pattern as MClash). TUN remains a mutually exclusive utun fallback (admin setuid). Both default off. MATCH stays the first group (`default` / `PROXY`). The **节点组** page lists cards; click a card or「添加节点组」opens a modal editor (live preview, source pills, chips, pin/exclude). Group `kind` is mihomo `select`（手动选择）, `fallback`（自动切换，按列表顺序探测）, or `url-test`（延迟最低）. Pins/`include` come first in pin order as fallback/select priority; `name_excludes` still only drops automatic matches. The **规则** page lists match/via in a table: click a row to edit in the composer (进程/域名/后缀/关键字/网段), right-click for move/via/delete. The **连接** page polls the local mihomo controller while visible (process, destination, chain, bytes, live up/down); empty when the core is disconnected. macOS keeps a menu-bar extra (left-click opens the window; right-click shows status plus connect or disconnect, apply, and quit). Developer mode and launch toggles (开机默认启动 / 静默启动 / 轻量模式 / 启动时默认连接) live on **设置**. Lite mode skips the main window until the menu-bar extra opens it. Levels: error / warn / info always go to `~/Library/Application Support/myproxy/myproxy.log`; debug / trace only with developer mode or `MYPROXY_DEV=1`. Lines are `HH:MM:SSZ level target message`. Subscription URLs are not logged. `myproxyctl log` prints the tail. `myproxyctl tun on|off` writes the TUN flag. `myproxyctl extension on|off` writes the System Extension flag.

Public repo: https://github.com/leaperone/myproxy. Stable Sparkle channel: `https://github.com/leaperone/myproxy/releases/latest/download/appcast.xml`. v0.0.1 is a full zip; later tags run `generate_appcast` against previous zips for deltas.

```sh
scripts/fetch-mihomo.sh
scripts/fetch-sparkle.sh
scripts/dev.sh                 # local watchexec restart
scripts/install-macbook-air.sh # signed /Applications app on macbook-air, mixed-port 7891
scripts/dev-air.sh             # refused; use install-macbook-air.sh
scripts/package-macos-app.sh
scripts/release-macos.sh
cargo run --bin myproxyctl -- capabilities
```
