# Findings

- 本机 36.1 接管失败：`configuration is too large (2506840 / 524288)`。Safari `gfw:Default` 把 GFWList 展开进 snapshot，再复制进 DNS bootstrap。
- 接管后 mihomo 看到的是扩展进程，PROCESS-NAME 不能钉 Safari；NE 必须做进程→入口。
- mihomo listener 支持 `rule: <sub-rule>`；`IN-NAME,a/b,DIRECT` 可作兜底。
- `via_target("gfw:Default")` 会拆成组名；域名 GFW 现编为 `AND` + `RULE-SET`。
- Host snapshot 去掉 dest / GFW 展开后只保留进程钉 + 默认 profile-rules。
- dest/`gfw:` 仅 AND+RULE-SET 时，未命中会落到后续 MATCH/组；需补裸条件 → DIRECT。
- cidr+`gfw:` 仍被 `Strategy::validate` 拒绝，运行时走不到 unwrap。
