# myproxy

GPUI controller for a bundled mihomo core. Canonical rules: `~/project/vibe/AGENTS.md`.

Strategy JSON under `~/Library/Application Support/myproxy/` is authoritative. Groups are source + name-contains (OR) + pins; an empty condition group stays empty. The UI page **规则** lists match/via in a table: click a row to edit in the composer, right-click for move/via/delete. Application matches currently compile to mihomo `PROCESS-NAME` and only apply after traffic enters Mixed port. macOS keeps a menu-bar extra (left-click opens the window, right-click for connect/apply/quit).

```sh
scripts/fetch-mihomo.sh
scripts/dev.sh                 # local watchexec restart
scripts/install-macbook-air.sh # release .app on macbook-air, mixed-port 7891
scripts/dev-air.sh             # compile here, copy binaries, relaunch Air
cargo run --bin myproxyctl -- capabilities
```
