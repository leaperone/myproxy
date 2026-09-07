# 调研与结论：app-logging

- 任务 ID：`app-logging-2026-09-07_18-55-42`
- 创建时间：`2026-09-07_18-55-42`

## 需求事实

- 用户确认：检查更新可用，但当前 `/Applications` 是本地 Dev 包，通道默认 Prod，需设 Nightly
- 用户要求：完整日志系统，带 Info / Warning / Error

## 真实调用链

- Rust：`log::emit` → `~/Library/Application Support/myproxy/myproxy.log` + 内存 ring
- UI：`developer_panel` 仅在开发者模式显示路径与 `log::recent`（ring，看不到 NE）
- `myproxyctl log` 读文件尾 80 行
- Host / NE：`os.Logger` → Console.app，不进 `myproxy.log`

## 调研结论

- Rust 分级已存在；缺口是设置页可见性、跨进程写入、文件作为 UI 真相源
- Host 与 app 同进程，但 NE 是独立进程，必须走文件

## 技术决策

| 决策 | 证据 |
|---|---|
| 文件尾作 `recent()` 真相源 | NE 行只在文件里；`myproxyctl log` 已这么做 |
| `stamp()` = generation + 文件长度 | 设置页轮询才能看到 NE 新行 |
| Shared `AppLog.swift` 仅 Host 使用 | NE sandbox 的 Application Support 不是用户目录 |

## 风险与边界

- 每条 flow 打 info 会撑爆日志，只记生命周期
- flock 竞争只在 rotate 窗口，可接受

## 参考指针

- `src/log.rs`、`src/ui.rs` `developer_panel`
- `macos/NetworkHost/HostBridge.swift`
- `macos/NetworkExtension/TransparentProxyProvider.swift` `startProxy` / `stopProxy`
