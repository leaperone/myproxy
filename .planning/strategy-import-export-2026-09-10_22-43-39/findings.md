# 调研与结论：strategy-import-export

- 任务 ID：`strategy-import-export-2026-09-10_22-43-39`
- 创建时间：`2026-09-10_22-43-39`

## 需求事实

- 仓库没有 export/import、文件选择器或备份命令。
- 权威配置是 `Strategy`（`~/Library/Application Support/myproxy/strategy.json`）。
- `origin/main`（`56800da`）已有 `load_from`、`validate`、`paths::atomic_write`、`MYPROXY_DATA_DIR`、`global_selected`。

## 真实调用链

- 导出：内存/load 的 Strategy → validate → pretty JSON → `atomic_write`
- 导入：`parse_import`（对象检查 + migrate + validate）→ `strategy.json.bak-import-*` → `save`
- GUI 导入后同步 login item、developer、Sparkle 通道；wanted 时 `start_apply`
- 文件面板：macOS `osascript` choose file / choose file name

## 调研结论

- catalog / runtime / sub-cache 不导出；`apply` 可重建。
- 实现必须在独立 worktree；主仓 `main` 有 Sparkle 未提交改动。
- `.agents/skills/agent/SKILL.md` 与 `.cursor/skills/agent/SKILL.md` 是同一 inode。

## 技术决策

| 决策 | 证据 |
|---|---|
| 复用 `atomic_write` | `Strategy::save` 已走同一路径 |
| `parse_import` 先于备份 | 坏 JSON / 非对象不碰本机文件 |
| 单测用临时目录 | 不设置 `MYPROXY_DATA_DIR`，避免并行测试抢环境变量 |

## 风险与边界

- 导出文件含订阅 URL；日志和 CLI JSON 只报路径与数量。
- 导入覆盖端口、系统接管、启动开关；回退靠 bak-import。
- 文件面板仅 macOS；Linux / Agent 走 CLI 路径。

## 参考指针

- `src/strategy.rs` `export_to` / `parse_import` / `import_from`
- `src/file_dialog.rs`
- `src/ui.rs` `strategy_backup_panel`
- `src/bin/myproxyctl.rs` `Export` / `Import`
