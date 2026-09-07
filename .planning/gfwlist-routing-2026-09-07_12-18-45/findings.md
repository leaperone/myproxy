# 调研与结论：gfwlist-routing

- 任务 ID：`gfwlist-routing-2026-09-07_12-18-45`

- 当前链：LAN DIRECT → `rule_sets` → `MATCH,unmatched_via`。未匹配在设置页。
- 入站四档只在 `rule` 时走这条链。
- mihomo 启动只有 `-f`，rule-provider `path` 需要 `-d` HomeDir。
- Loyalsoldier `gfw.txt` 可直接 `behavior: domain`。
