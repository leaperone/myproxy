# Findings

- macmini 39.1：NEDNSProxy 已拦系统解析，`scutil` 仍显示 `1.1.1.1`/`8.8.8.8`。`:1053` 一停，`getaddrinfo` 整机超时。
- `disconnect_inner` 原先只 `disable_async`（提交）就立刻杀 mihomo。CLI 关 DNS 常遇 `NEDNSProxyErrorDomain 1`。
- `origin/main` 的 `disable()` 用 `disableDNSProxyAllowingDenied`：denied 被吞掉后把 `dnsPhase` 标成 disabled，`wait_disabled` 会误判已交还系统 DNS。
- DNS TCP/UDP `unavailableFallback` 原先为 `.reject`，SOCKS/` :1053` 不可用时查询被关掉。扩展进程直连上游不受 NEDNSProxy 拦截。
- `shutdown` 才 `wait_disabled`；普通 disconnect/apply 重连不会等。
- 关/开系统接管必须走已签名 `.app` 窗口；CLI 缺 `system-extension.install` entitlement。
