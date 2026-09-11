# 执行进度：ui-ux-review-fixes

- 任务 ID：`ui-ux-review-fixes-2026-09-11_19-07-30`
- 创建时间：`2026-09-11_19-07-30`
- 当前状态：`in_progress`

## 已完成

- 只读探索：确认主题 token 解析链、卡片交互缺口、对比度实测值与 grep 证据（见 findings.md）。
- 安全 checkout：`fix/ui-ux-review-fixes` @ `bc0768f`，worktree `.worktrees/fix-ui-ux-review-fixes`；基线校验通过（AGENTS.md 实体、CLAUDE.md 符号链接、`.worktrees/` 已忽略、`.planning/` 收录）。
- planning 三文件建立。
- 新增 `src/theme_ext.rs`：`warning_text` / `chip_bg` / `chip_fg` / `selection_border` / `row_hover` / `luminance` / `contrast_ratio` / `separates`，并在 `lib.rs` 暴露。
- 全部告警文字改走 `theme_ext::warning_text`（页面状态、标题栏、编辑器 notice、扩展消息、订阅警告、代理错误、日志面板）。
- chip 改中性底 + 前景字：`pill` 去掉颜色参数，调用点 4 处；卡片选中态改由 `selection_border` 描边表达。
- 悬停底改 `theme_ext::row_hover`（组卡/规则卡/GLOBAL 卡/连接行），修掉浅色下与卡片底同值导致的无反馈。
- 连接页筛选表头激活色由 `accent` 改 `primary`；组成员「当前」底色改 `chip_bg`。
- 规则卡补「编辑 / 上移 / 下移」行内按钮（带可访问名），与既有「删除」并列。
- 删除与「打开系统设置」重复的第二个 `open-login-items` 按钮（id 重复）。
- 总览删掉与 hero 重复的「状态」指标；`kept` → `保留`；断开不再用 danger 语义；托盘「更新配置」→「应用」。
- `AGENTS.md` 补两句契约：规则卡行内操作按钮、UI 文字色走 `theme_ext` 角色。

## 进行中

- 交付前收敛检查与 PR 流程（preflight 五门：构建/冲突探测/领域检查均已 pass）。

## Preflight 证据

- Phase 1 构建：Rust scope（`src/`）`cargo check --quiet` exit=0；`cargo build --quiet --bins --features sparkle` exit=0（worktree 缺 `resources/sparkle/Sparkle.framework`，从主 checkout 复制同一份 fetch 产物后构建）。
- Phase 1.5：`git merge-tree --write-tree HEAD origin/main` 干净，0 条 CONFLICT（`origin/main` 为 HEAD 的祖先）。
- Phase 2 领域检查：隔离 `MYPROXY_DATA_DIR` 后 `cargo test --quiet --lib`（按配置跳过 2 例）74 passed。
- Phase 3 审查：环境无 review-agent skill，由主 agent 直接审查 diff；0 critical / 0 high，4 low（日志面板重复计算 warning_text、`separates` 仅供测试、深色悬停仍偏弱、卡片点击与行内编辑冗余），均不阻塞。

## 修改文件

- `.planning/ui-ux-review-fixes-2026-09-11_19-07-30/{task_plan,findings,progress}.md`
- `src/theme_ext.rs`（新增，含 5 个单测）
- `src/lib.rs`、`src/ui.rs`、`src/tray.rs`
- `AGENTS.md`

## 验证结果

| 检查 | 结果 | 状态 |
|---|---|---|
| 主题 token 解析链 | `Theme` Deref → `ThemeColor`；`accent` = `accent.background`（悬停底语义）；`group_box`/`muted` 浅色同值 | 通过 |
| 对比度实测（源码 + 主题取值） | accent 作字 Light ≈1.0:1 / Dark ≈1.3:1；warning 作字 Light ≈1.8:1 | 通过（问题确认） |
| 键盘通路 grep（改动前） | `track_focus/.role(/tab_index` 无匹配；无 `KeyBinding` | 通过（缺口确认） |
| `cargo test --lib theme_ext` | 5 passed（含用已知 4.74:1 对照校验 WCAG 计算） | 通过 |
| `cargo test` | 76 passed / 0 failed；3 个 bin 测试 0 | 通过 |
| `cargo check --bins` | Finished，无新增警告 | 通过 |
| 回归 grep | `text_color(theme.accent)` / `theme.accent` / `text_color(theme.warning)` 在 `src/ui.rs` 均为 0；`open-login-items` 1 处；`kept` 0 处；规则卡 `edit-rule-/up-rule-/down-rule-` 各 1 处 | 通过 |
| `cargo clippy` | 工具链 1.98.1 未安装 clippy（`'cargo-clippy' is not installed`），未运行 | 未验证 |
| 渲染眼验 | 本机 release 实例在跑 + 窗口在屏外，截图不可用 | 未验证 |

## 错误与恢复

| 错误 | 尝试 | 解决方式 |
|---|---:|---|
| `git rev-parse --short main origin/main` 退出 128 | 1 | 分开验证：`origin/main` 与本地 `main` 同为 `bc0768f`，base 正常 |
| 窗口无法截图（`screencapture -R` 只得到 2×56） | 1 | AX 报告窗口在屏外（`1469,928`，本机 1280×832）；改用 token 取值 + 对比度单测验证，并在 findings 记录为未眼验风险 |
| `theme_ext.rs` 首次 `cargo test` 报 `unresolved import gpui` | 1 | 本项目通过 `gpui_kit::gpui` 重导出，改为 `use gpui_kit::gpui::{hsla, Hsla}` 与测试内 `gpui_kit::gpui::rgb` |
| 一次 `edit` 批量替换因最后一条 oldText 不匹配整体失败 | 1 | 按实际源码重写该条（重复按钮的删除块），其余 22 条一并重放 |
