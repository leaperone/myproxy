# myproxy

GPUI controller for a bundled mihomo core. Canonical rules: `~/project/vibe/AGENTS.md`.

Strategy JSON under `~/Library/Application Support/myproxy/` is authoritative. The UI page **规则** is domain/app routing (not a copy of MClash App Routing). Application matches currently compile to mihomo `PROCESS-NAME` and only apply after traffic enters Mixed port.

```sh
scripts/fetch-mihomo.sh
scripts/dev.sh          # watchexec restart on save
cargo run --bin myproxyctl -- capabilities
```
