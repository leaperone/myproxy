# 调研与结论：ui-ux-review-fixes

- 任务 ID：`ui-ux-review-fixes-2026-09-11_19-07-30`
- 创建时间：`2026-09-11_19-07-30`

## 需求事实

- 前一轮只读评审（better-interface full）对 `src/ui.rs`（6175 行）+ `src/tray.rs` + `src/main.rs` 给出 14 项发现：3 HIGH、8 MEDIUM、2 LOW。
- 本任务取 HIGH 全部 + 不改变行为契约的一致性缺陷；设置页 IA、文案密度、选择器搜索等留后续。
- 工作树基线：`bc0768f`（2026-09-11 01:02），`fix/ui-ux-review-fixes` 分支，base `origin/main`（同一 commit）。
- 本机现状：`/Applications/myproxy.app`（0.0.8-nightly.20260910.42.1）正在运行，mihomo 与 NetworkExtension 均在跑，单实例守卫会拒绝第二个实例。

## 真实调用链

- 主题来源：`gpui_kit::init` → `gpui-component theme::init` → `Theme::change(mode)` → `DEFAULT_THEME_COLORS`（内置 `default-theme.json` 的 Default Light / Default Dark）。`appearance.rs` 只切 mode，不装载自定义主题；本机无自定义主题目录。
- token 解析：`Theme` Deref → `ThemeColor`（`theme/mod.rs:155`）；`ThemeColor.accent` 由 schema 键 `accent.background` 填充（`schema.rs:251`、`904`），注释写明用途是「hover background on MenuItem/ListItem」（`theme_color.rs:55`）；`ThemeToken` Deref 到 `color`，`.bg()` 走 `background`。因此 `theme.accent` 是**背景类**取值。
- 使用点：`theme.accent` 在 `ui.rs` 里被当文字色与选中色（`pill` 的 `text_color(color)`、卡片 `border_color(if selected { accent … })`、`bg(accent.opacity(0.14))`、筛选表头激活色）；`theme.muted` 被当悬停底；`theme.warning` 被当文字色。
- 卡片交互：`render_group_card` / `render_rule_set_card` / `render_global_card` / `render_connection_row` 都是 `div()` + `.on_click()`，无 `track_focus` / `.role(` / `tab_index`（全文件 grep 无匹配）；规则卡的编辑与上移/下移只存在于右键菜单（`ui.rs:6008`、`6015`、`6023`）。
- 对照：`gpui-component` 的 Button 会 `track_focus` + `tab_index` + `tab_stop`（默认 true）并设 `Role::Button`、`accessibility_label` 缺省取可见标签（`button/button.rs:735-737`、`654`）；Switch 同理（`switch.rs:118-178`）。缺口只在自绘卡片。

## 调研结论

1. 浅色下 `group_box`(#f5f5f5) 与 `accent`(neutral-100 = #f5f5f5)、`muted.background`(neutral-100) 同值 → 选中底、选中描边、卡片悬停反馈全部不可见；深色下 `accent`(neutral-800) 与 `border`(neutral-800) 同值 → 选中与普通描边无法区分。
2. `pill` 用 `color.opacity(0.16)` 作底、`color` 作字：accent 文字在浅色 ≈1.0:1、深色 ≈1.3:1（12px 正文需 4.5:1）。
3. `warning` 作文字：浅色 `yellow-500` 落在 `#f5f5f5` ≈1.8:1；深色 `yellow-400` 反而 ≈14:1。主题未提供「表面上的告警文字」角色：`warning_foreground` 浅色是 neutral-50（仅适合黄色填充面）。
4. `warning_foreground` 也不能直接用于深色填充：深色 `yellow-600` 落在 `yellow-400` 填充上 ≈1.9:1。
5. 中性 chip（`secondary` 底 + `foreground` 字）两模式都安全：浅色 (neutral-200, neutral-900) ≈16:1，深色 (neutral-800, neutral-50) ≈14.5:1。
6. `status_dot` 的绿色（green-500 对 #f5f5f5 ≈2.1:1）低于非文本 3:1，但圆点旁始终有文字标签，属冗余提示，不构成阻断。

## 技术决策

| 决策 | 证据 |
|---|---|
| token 角色助手放 lib 层并带对比度单测 | 需要按 Light/Dark 计算并断言，bin 内联无法单测 |
| chip 改中性底 + 前景字 | 结论 2、5：accent 无法作前景；状态语义交由文字承载 |
| 告警文字按模式混色 | 结论 3、4：主题既有 token 无可用角色，需向 foreground 混色到 ≥4.5:1 |
| 选中态改用 `primary`/`ring` 类前景+描边组合 | 结论 1：accent 与卡片底/边框同值，无法表达选中 |
| 规则卡补 Button 而非自绘焦点 | 调用链证据：Button 已具备 role/tab_stop/focus ring |

## 风险与边界

- 视觉未眼验：本机只有一块 1280×832 逻辑分辨率屏幕，AX 报告 myproxy 窗口位于 `1469,928`（屏外），`screencapture -R` 只得到 2×56 裁切图；且 release 版在跑，单实例守卫会拒绝调试构建。因此本次以「token 取值 + 对比度计算 + 单测」代替眼验。
- 被评审源码（`bc0768f`）晚于运行中的安装版本（`0.0.8-nightly.20260910.42.1`），截图即使拿到也不能代表当前源码。
- 改 chip 观感属设计取舍：功能不变、颜色更克制，但视觉风格变化需在 PR 里确认。
- 不动 AGENTS.md 契约（点卡片打开编辑、空组 REJECT、NE/mihomo 分工），避免行为回归。

## 参考指针

- 评审结论（本任务前一轮只读输出）：14 项发现，含每项的行号与对比度计算。
- `gpui-component-0.6.0`：`src/theme/mod.rs:155`、`src/theme/theme_color.rs:11-62`、`src/theme/schema.rs:251`、`904`、`src/theme/registry.rs:12-33`、`src/button/button.rs:735-737`。
- 项目契约：`AGENTS.md` 总览页与规则页段落、README「Routing and runtime state」。
