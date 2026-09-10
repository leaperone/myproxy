# 调研与结论：se-clash-gaps

- 任务 ID：`se-clash-gaps-2026-09-10_23-53-43`
- 创建时间：`2026-09-10_23-53-43`

## 需求事实

按路线图补齐系统接管可见性与 Clash 缺口。

## 真实调用链

- Provider 已有 `.activity` / `relayLocalPort` / `AppRoutingActivityRing`
- Host `HostProviderControlResponse` 未带 `activityBatch`；Rust FFI 仅 enable/disable/status
- 连接页 `controller::fetch` → mihomo `/connections`，SE 中继后 process 是扩展
- `BuiltInBypassPolicy` 已放行私网/回环；设置页未列出
- `RuntimeStatus` 已有 phase / DNS / WaitingApproval；无 failOpen 与登录项按钮
- 无系统代理；分流无 chinadirect
- catalog 已有 `subscription_warning`，总览仍把 excluded 混在一起

## 调研结论

最小路径：瘦 JSON activity FFI + sourcePort join；旁路只读 + `lan_capture`；系统代理 `networksetup` + 恢复文件。

## 技术决策

| 决策 | 证据 |
|---|---|
| activity 瘦 JSON | Swift activity 图对 Rust 过重 |
| snapshot 不加 schema | `capturePrivateNetworks` 默认 false |
| GEOIP 只在 mihomo | 计划明确 NE 不做 IP 库 |

## 风险与边界

- activity FFI 在 background 线程同步等待 provider，超时 800ms
- 系统代理只覆盖已启用的 networksetup 服务

## 落地指针

- `macos/NetworkHost/TransparentProxyManager.swift` `fetchActivity`
- `macos/NetworkHost/HostBridge.swift` `myproxy_ne_activity_batch`
- `src/controller.rs` `LiveConnection::from_raw` / `fetch(..., system_extension)`
- `src/system_proxy.rs` `networksetup` + `system-proxy-restore.json`
- `src/compile.rs` `GEOIP,CN,DIRECT` 仅 chinadirect

## 参考指针

- `macos/NetworkShared/AppRoutingActivity.swift`
- `macos/NetworkHost/TransparentProxyManager.swift`
- `src/controller.rs` `LiveConnection::from_raw`
