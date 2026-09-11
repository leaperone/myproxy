# 任务计划：ui-ux-review-fixes

- 任务 ID：`ui-ux-review-fixes-2026-09-11_19-07-30`
- 创建时间：`2026-09-11_19-07-30`

## 目标

修复 UI/UX 评审（better-interface full 模式，本任务前一轮只读产出）中已确认的 HIGH 缺陷及其连带的一致性缺陷，使：

1. 状态/警告文字与选中态在浅色与深色两种外观下都可读（对比度达标，有单测固定）。
2. 规则卡的全部操作（编辑、上移、下移、删除）都能用键盘与读屏完成，且每个交互元素 id 唯一。
3. 文案与状态呈现的一致性缺陷（英文残片、重复状态、红色语义漂移、重复控件）修正。

成功标准：`cargo test --lib` 与 `cargo check --bins` 通过；对比度单测对 Light/Dark 默认主题取值均 ≥ 4.5:1（正文）且断言选中态与卡片底色可区分；规则卡每个操作都有可聚焦控件。

## 范围

- `src/ui.rs`：token 角色使用点（`pill`、卡片选中/悬停、告警文字、连接筛选激活色）与规则卡操作按钮。
- 新增 `src/theme_ext.rs`：按角色取色的助手（告警文字、中性 chip、选中描边、悬停底色）+ 对比度单测。
- `src/tray.rs`：一个术语统一。
- 交付物：`fix/ui-ux-review-fixes` 分支 + PR，planning 三文件随提交。

## 非目标

以下评审项明确留到后续任务，不在本 PR 内：

- 设置页信息架构重构（把 Mixed / 系统接管 / TUN / 系统代理 收进同一「入站与接管」分组）。
- 12px 长说明文案降密度与字号调整。
- 「走向」选择器补搜索。
- `self.status` 由中文字符串判断严重级别改为类型化状态。
- 破坏性操作（关闭全部连接、删除订阅/组/规则）一律补二次确认。
- Swift / NetworkExtension、`myproxyctl` 输出、打包与发布流程。

## 关键约束

- 不改变 AGENTS.md 已记录的对外行为契约：点击卡片打开编辑弹窗、空组用 REJECT 哨兵且不隐式直连、进程/dest 规则在 NE 与 mihomo 的分工。
- 只用 `cx.theme()` 语义 token，不引入第二套配色或自定义色板文件。
- 保持中文界面与既有术语（应用 / 断开 / 直连 / 拒绝）。
- 优先用 `gpui-component` 的 Button/Switch 能力（自带 `Role::Button`、`tab_stop`、focus ring、`accessibility_label` 缺省取可见标签），不手写焦点与无障碍状态。
- 不启动 GUI 验证：本机 release 版正在运行（单实例守卫会拒绝），且会触碰实时路由状态。

## 修改路径

| 文件 | 位置 | 改动 |
|---|---|---|
| `src/theme_ext.rs` | 新增 | `warning_text` / `chip_bg` / `chip_fg` / `selection_border` / `row_hover` / `contrast_ratio` + 单测 |
| `src/lib.rs` | `pub mod` 列表 | 暴露 `theme_ext` |
| `src/ui.rs` | `pill`（`ui.rs:6158`）、`render_group_card`（`5279`）、`render_global_card`（`5495`）、`render_rule_set_card`（`5955`）、`render_connection_row`（`5834`）、`connection_filter_header`（`5723`）、`render_member_row`（`5130`） | accent/muted 作文字与选中态 → 角色化 token |
| `src/ui.rs` | 告警文字点：`2742`、`2754`、`2844`、`3133`、`3229`、`3532`、`3614`、`4554`、`4555`、`5167` | `theme.warning` 作文字 → `theme_ext::warning_text` |
| `src/ui.rs` | `render_rule_set_card`（`5955`） | 卡片头补「编辑」「上移」「下移」按钮，唯一 id，去掉重复 id 与重复控件 |
| `src/ui.rs` | `4557` 附近的第二个 `open-login-items` | 删除重复按钮 |
| `src/ui.rs` | `3120` | `kept` → `保留` |
| `src/ui.rs` | `3102` | 删除与 hero 重复的「状态」指标 |
| `src/ui.rs` | `2884` | 断开不再用 danger 语义 |
| `src/tray.rs` | `105` | 「更新配置」→「应用」 |
| `AGENTS.md` | 规则页与连接页两句 | 记录规则卡行内操作按钮与 `theme_ext` 文字色约定 |
| `.planning/...` | 本目录 | 三文件 |

## 验证方式

- `cargo test --lib theme_ext`：默认主题 Light/Dark 取值下，告警文字对卡片底 ≥ 4.5:1；chip 前景对 chip 底 ≥ 4.5:1；选中描边与卡片底非同一色。
- `cargo test --lib`：全量单测不回归。
- `cargo check --bins`：两个 bin 编译通过。
- 回归 grep：`grep -n "text_color(theme.accent)" src/ui.rs` 为空；`grep -c "pill(theme" src/ui.rs` 与改动后一致（每个 pill 都走角色化 helper）。
- 键盘可达性：`grep -n "track_focus" src/ui.rs` 或规则卡操作按钮存在（Button 自带 tab_stop）。

## 验收标准

1. 浅色与深色外观下，告警/状态文字对比度 ≥ 4.5:1（单测断言）。
2. 规则卡的编辑、上移、下移、删除均有可聚焦控件，且元素 id 全局唯一。
3. 同一 panel 内不再有两个做同一件事的按钮。
4. 中文界面里不再出现 `kept` 之类的未翻译残片。
5. 总览页内只有 hero 一处状态；原重复的「状态」指标已删（标题栏状态属全局 chrome）。
6. `cargo test --lib`、`cargo check --bins` 通过。

## 未确认事项

- 视觉观感未做眼验（本机截图受限，见 findings 风险节）；彩色 chip 改中性底是本次的设计取舍，需在 PR 里由用户确认。
- 设置页 IA 重构、12px 长文案降密度、「走向」选择器搜索、`status` 分级类型化、破坏性操作二次确认：已列为非目标，留后续任务。
- 告警文字的具体色值由对比度约束推导（浅色下向 foreground 混色到 ≥ 4.5:1），非视觉选色。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 新建 `src/theme_ext.rs` 收敛 token 角色 | 10+ 调用点需要同一角色判断；放 lib 层才能单测对比度 |
| 彩色 chip 改中性底 + 前景文字 | `accent` 是悬停背景 token，浅色下与卡片底同值；状态改由文字承载，符合「不靠颜色单独表意」 |
| 告警文字改为按模式混色到达标 | 主题只提供填充色 `warning` 与仅适合填充面的 `warning_foreground`，浅色下前者 1.8:1、深色下后者 1.9:1 |
| 规则卡补显式操作按钮而非自定义焦点 | Button 已带 role/tab_stop/focus ring/可访问名，避免手写焦点与键盘状态 |
| 保留「点击卡片打开编辑弹窗」 | AGENTS.md 记录为既定行为，本次只补键盘通路 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| | 1 | |
