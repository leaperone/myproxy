# Progress

- 工作已在 `fix/se-dns-fail-open`（base `ce366ae`）。
- disconnect 先证明 capture+DNS 已关；关不掉则保留核心。
- 全量 disable 不再吞 `NEDNSProxyErrorDomain 1`。
- DNS TCP/UDP fallback 为 `.direct`。
- `core_stops_only_after_capture_and_dns_are_down` 通过；supervisor 其余单测通过。
