# 任务计划：system-extension-dns

## 目标
让 macOS 系统接管（NETransparentProxy）模式同时启用现有 NEDNSProxyProvider 链路，并为 mihomo 编译可供 DNS Proxy 使用的 DNS 模块，使系统解析进入策略。

## 范围
- host 启用/停用路径接入 NEDNSProxyManager，并向 HostBridge 发送 DNS proxy bootstrap 配置。
- system extension 模式 runtime YAML 编译 DNS 配置（不启用 TUN 专属 dns-hijack）。
- 补充必要测试与失败回滚，保持现有 TUN/Mixed 行为。

## 非目标
- 不改用 TUN 或修改系统 DNS 地址为 127.0.0.1。
- 不重构 NEDNSProxyProvider 内部转发策略，不改变用户策略匹配语义。

## 关键约束
- 遵守仓库 AGENTS.md 与 standard-development；使用独立 worktree。
- DNS proxy 启用失败必须恢复系统解析器并报告失败。
- 不记录订阅 URL。

## 修改路径
- host/Swift 或 Rust bridge 中 DNS proxy manager 生命周期与 bootstrap。
- `src/compile.rs` 及相关 runtime 配置。
- 单元/集成测试与本目录 planning 文件。

## 验证方式
- `cargo test --lib`、`cargo check --quiet`、`git diff --check`。
- 针对 system_extension=true 的 runtime YAML 断言包含 dns 且不含 dns-hijack。
- HostBridge/manager 失败路径静态与编译验证。

## 验收标准
- 开启系统接管后 NEDNSProxyManager 与透明代理一起启用，传入 mihomo SOCKS/端口等 bootstrap。
- system extension runtime 启用 dns.listen、nameserver 等配置，DNS proxy 可将请求交给 mihomo；TUN 仍使用 dns-hijack。
- 任一 manager 启用失败时回滚已启用的 manager，系统解析恢复。
- 现有测试通过且无未提交无关改动。

## 未确认事项
- macOS 原生扩展现场验证受当前环境限制。

## 执行状态
- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策
| 决策 | 理由 |
|---|---|
| 复用现有 NEDNSProxyProvider 并从 host 协调启用 | 仓库已有扩展实现，缺口在应用侧 manager 生命周期 |
| SE 模式仅开启 mihomo DNS listener，不加入 dns-hijack | dns-hijack 依赖 TUN；DNSProxyProvider负责系统解析接管 |

## 错误与处理
| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| | | |
