# myproxy

GPUI controller for a bundled mihomo core. Canonical rules: `~/project/vibe/AGENTS.md`.

Strategy JSON under `~/Library/Application Support/myproxy/` is authoritative. Groups are source + name-contains (OR, `*` `?` wildcards) + pins − exact exclude; `name_excludes` drops automatic matches only. Empty condition groups stay empty. The **节点组** page lists cards; click a card or「添加节点组」opens a modal editor (live preview, source pills, chips, pin/exclude). The **规则** page lists match/via in a table: click a row to edit in the composer, right-click for move/via/delete. Application matches currently compile to mihomo `PROCESS-NAME` and only apply after traffic enters Mixed port. macOS keeps a menu-bar extra (left-click opens the window, right-click for connect/apply/quit). Developer mode lives on **设置**; it writes `~/Library/Application Support/myproxy/myproxy.log` (`MYPROXY_DEV=1` forces it on; `myproxyctl log` prints the tail).

```sh
scripts/fetch-mihomo.sh
scripts/dev.sh                 # local watchexec restart
scripts/install-macbook-air.sh # release .app on macbook-air, mixed-port 7891
scripts/dev-air.sh             # compile here, copy binaries, relaunch Air
cargo run --bin myproxyctl -- capabilities
```
