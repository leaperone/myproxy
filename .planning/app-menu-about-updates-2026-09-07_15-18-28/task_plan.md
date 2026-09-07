# 任务计划：app-menu-about-updates

- 任务 ID：`app-menu-about-updates-2026-09-07_15-18-28`
- 创建时间：`2026-09-07_15-18-28`

## 目标

点 macOS 菜单栏上的 MyProxy 应用名时，下拉菜单包含 About 和 Check for Updates。

## 范围

- 在 `src/main.rs` 用 GPUI `set_menus` 安装应用菜单，并注册 About / Check for Updates / Quit 动作。
- About 打开系统标准关于面板；Check for Updates 走现有 Sparkle；Quit 先停核心再退出。
- 托盘「关于」改为同一关于面板。
- 更新 AGENTS.md 里菜单栏行为一句。

## 非目标

- 不改侧栏、连接页、发布通道或 Sparkle 桥本身。
- 不重做托盘图标的左键逻辑。

## 关键约束

- 主仓不写代码；只在 `feat/app-menu-about-updates` worktree 改。
- 无窗口时全局 `on_action` 仍要能点菜单项。
- 开发构建没有 Sparkle 时，「Check for Updates」禁用。

## 修改路径

- `src/main.rs`
- `src/tray.rs`
- `AGENTS.md`

## 验证方式

- `cargo check --bin myproxy`
- 对照 `set_menus` / `on_action` 接线

## 验收标准

- 应用菜单有 About MyProxy、Check for Updates…、Quit MyProxy。
- About 打开标准关于面板，不是只打开主窗口。
- Check for Updates 调用 `sparkle::check`；无 Sparkle 时该项禁用。

## 未确认事项

无。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [ ] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 用 GPUI `set_menus` 而不是改托盘 | 用户要的 About / Check Update 是左侧应用菜单；托盘已有中文项 |
| About 走 `orderFrontStandardAboutPanel` | 系统标准关于窗，不用再做一页 UI |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| | 1 | |
