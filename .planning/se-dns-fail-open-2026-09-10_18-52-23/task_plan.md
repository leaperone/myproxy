# 任务计划：se-dns-fail-open

- 任务 ID：`se-dns-fail-open-2026-09-10_18-52-23`
- 创建时间：`2026-09-10_18-52-23`

## 目标

系统接管关闭或核心未就绪时，NEDNSProxy 不得把系统解析卡死。

## 范围

- `disconnect` 先等接管和 DNS 真正关掉，再停 mihomo
- DNS 中继在 SOCKS 不可用时直连上游，而不是 reject
- 仅在即将重新启用 DNS 时容忍 `stopDNSProxy` permission denied

## 非目标

- 不改规则页 / 节点组语义
- 不在本任务发 Nightly

## 关键约束

- 未证明系统 DNS 已交还 macOS 时，必须保留 `:1053`
- 不打印订阅 URL 或 NE 凭证

## 修改路径

- `src/supervisor.rs`
- `src/network_extension.rs`
- `macos/NetworkHost/HostBridge.swift`
- `macos/NetworkExtension/DNSProxyProvider.swift`
- `AGENTS.md`
- `README.md`

## 验证方式

- `cargo test --lib core_stops_only_after_capture`
- `cargo test --lib supervisor::`（跳过依赖本机 RuntimeConfig 的 hook 测试）

## 验收标准

- disconnect 在 wait_disabled 成功前不 reclaim 核心
- DNS TCP/UDP `unavailableFallback` 为 direct
- enable 重启路径仍能在 disable denied 后写入新 DNS 配置

## 未确认事项

无。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 关接管失败则保留核心 | NEDNSProxy 仍拦截时杀掉 `:1053` 会整机超时 |
| DNS 后端不可用则直连上游 | 扩展进程出口不受 NEDNSProxy 拦截 |

## 错误与处理

无。
