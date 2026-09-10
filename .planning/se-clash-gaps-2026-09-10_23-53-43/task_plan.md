# 任务计划：se-clash-gaps

- 任务 ID：`se-clash-gaps-2026-09-10_23-53-43`
- 创建时间：`2026-09-10_23-53-43`

## 目标

接通系统接管 activity / 状态通道，让连接页显示真实进程并可从连接建规则；补可见旁路、可选系统代理、国内直连分流、订阅失败与过滤排除分离。

## 范围

- activity FFI + `relayLocalPort`/`sourcePort` join
- 连接行右键建规则 / 改走向
- 设置页扩展状态、登录项按钮、只读旁路、局域网进规则
- 可选系统 HTTP/SOCKS 指向 Mixed，断开恢复
- `RoutingProfile::Chinadirect`
- catalog 拉取失败与 exclude_filter 分列

## 非目标

- 外部 SOCKS 链、Clash YAML、跨平台、用 activity 替换连接页、NE 内嵌 GEOIP

## 关键约束

- mihomo 行仍是连接页主数据
- 安全旁路不可删除
- GEOIP 只在 mihomo 层生效
- 系统接管关闭时不调用 activity FFI

## 修改路径

- `macos/NetworkHost/*`、`macos/NetworkShared/Capture*`
- `src/network_extension.rs`、`src/controller.rs`、`src/ui.rs`
- `src/strategy.rs`、`src/compile.rs`、`src/catalog.rs`、`src/system_proxy.rs`
- `src/supervisor.rs`、`src/bin/myproxyctl.rs`

## 验证方式

- `cargo test --offline --lib --bin myproxyctl --tests` 覆盖 join、chinadirect 编译、catalog 计数、SE 关跳过 FFI
- 设置页与连接页文案对照路线图

## 验收标准

- SE 开时连接进程可被 activity 覆盖；关时不调 FFI
- 连接可建进程规则
- 设置能看到旁路与扩展状态
- 系统代理可开可恢复
- 国内直连发出 `GEOIP,CN,DIRECT` + MATCH 默认组
- 总览区分拉取失败与过滤排除

## 未确认事项

无

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| activity FFI 返回瘦 JSON | 避免 Rust 反序列化完整 Swift activity 图 |
| `capturePrivateNetworks` 不升 snapshot schema | `decodeIfPresent` 默认 false，旧快照仍可用 |
| 系统代理走 `networksetup` + 恢复文件 | 退出/断开可还原，不做守卫轮询 |
| `fetch(mixed_port, system_extension)` | 连接页才 join；SE 关直接空 map，不进 FFI |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| 连接行 `via_menu` 闭包移出 `Fn` 捕获 | 1 | 在 submenu 与 on_pick 内再 clone |
| `controller::fetch` 半截替换破坏函数体 | 1 | 重写为带 `system_extension` 的完整函数 |
