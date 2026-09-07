# 任务计划：app-logging

- 任务 ID：`app-logging-2026-09-07_18-55-42`
- 创建时间：`2026-09-07_18-55-42`

## 目标

把现有 `myproxy.log` 做成应用级分级日志：Info / Warning / Error 始终可见、可打开；Host 生命周期写入同一文件。

## 范围

- 扩展 `src/log.rs`：统一行格式、文件锁、从文件读尾
- 设置页独立「日志」面板（分级说明、路径、Finder、尾部着色）
- NetworkShared `AppLog`；Host 启停 / 审批 / 失败打 info/warn/error
- AGENTS 补一句：设置页始终展示 info/warn/error，Host 写同一文件

## 非目标

- 不换日志框架、不做遥测
- 不记录订阅 URL
- 不把 debug/trace 默认打开
- 不记录每条 NE flow
- 不混入主仓 Mixed 调试改动
- 不改 DNS 接管策略

## 关键约束

- 行格式保持 `HH:MM:SSZ level target message`
- error/warn/info 始终落盘；debug/trace 仅开发者模式或 `MYPROXY_DEV=1`
- Host 与 app 同进程，写 `Application Support/myproxy/myproxy.log`；NE sandbox 不写该文件

## 修改路径

- `src/log.rs`、`src/ui.rs`、`AGENTS.md`
- `macos/NetworkShared/AppLog.swift`
- `macos/NetworkHost/HostBridge.swift`、`SystemExtensionClient.swift`
- 不改 Network Extension provider（沙盒写不到用户日志）

## 验证方式

- `cargo test --lib log::`
- `cargo check --bins`
- 对照设置页「日志」是否不依赖开发者模式

## 验收标准

- 设置页不开启开发者也能看 Info/Warning/Error 尾部并在 Finder 中显示
- Host 启用/停用/审批/失败写入同一 `myproxy.log` 且格式一致
- debug/trace 仍仅开发者模式

## 未确认事项

无

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [ ] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 沿用现有 `log.rs`，不引入 tracing | 已有分级与文件；缺口在可见性与 Swift 进程 |
| Host 写同一 `myproxy.log` + flock | Host 与 app 同进程，能写 Application Support；NE 有 sandbox |
| 设置页「日志」与「开发者」拆开 | Info/Warning/Error 是产品能力，不是开发者开关 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| NE sandbox 写不到 Application Support | 1 | 去掉 provider AppLog，改由 Host 记生命周期 |
