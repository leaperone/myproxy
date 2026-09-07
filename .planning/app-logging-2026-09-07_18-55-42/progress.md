# 执行进度：app-logging

- 任务 ID：`app-logging-2026-09-07_18-55-42`
- 创建时间：`2026-09-07_18-55-42`
- 当前状态：`in_progress`

## 已完成

- 只读确认：Rust 已有分级；设置页日志藏在开发者开关后；Swift 不写 `myproxy.log`
- `log.rs`：格式化、换行清洗、flock、文件尾、`stamp()`、单测
- 设置页「日志」面板始终可见；开发者开关只控制 debug/trace
- Shared `AppLog.swift`；Host / NE 生命周期 info/warn/error
- Host 与 Network Extension 已编译通过

## 进行中

- 交付：commit / PR / preflight

## 修改文件

- `src/log.rs`、`src/ui.rs`、`AGENTS.md`、`README.md`
- `macos/NetworkShared/AppLog.swift`
- `macos/NetworkHost/HostBridge.swift`、`SystemExtensionClient.swift`
- `macos/NetworkExtension/TransparentProxyProvider.swift`、`DNSProxyProvider.swift`

## 验证结果

| 检查 | 结果 | 状态 |
|---|---|---|
| `cargo test --lib log::` | 3 passed | 通过 |
| `cargo check --bins` | ok | 通过 |
| `scripts/build-network-host.sh` | built | 通过 |
| `scripts/build-network-extension.sh` | built | 通过 |
| rustfmt | toolchain 无 rustfmt | 跳过 |

## 错误与恢复

| 错误 | 尝试 | 解决方式 |
|---|---:|---|
| migrate-session 未把 planning 落到 worktree | 1 | 手工写入 worktree 三文件 |
| rustfmt 未安装 | 1 | 手工格式，不装组件 |
