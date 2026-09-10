# Progress

- inbound_plan 只钉进程；`gfw:` 走 `gfw_ports`，不再下发 dest / GFW 域名。
- compile 增加 GFW 入口 listener（无 `proxy:`，有 `rule`/`sub-rules`）和 IN-NAME 兜底。
- 域名 `gfw:` 编成 `AND` + `RULE-SET`；有 `gfw:` 或 GFWList 时写入 rule-provider。
- HostBridge snapshot 只保留进程规则 + 默认 profile-rules。
- 验证：compile 20、network_extension 4，均通过。
