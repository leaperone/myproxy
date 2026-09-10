# 任务计划：strategy-import-export

- 任务 ID：`strategy-import-export-2026-09-10_22-43-39`
- 创建时间：`2026-09-10_22-43-39`

## 目标

给 myproxy 补齐策略配置导入/导出：以权威 `strategy.json` 为唯一交换格式，提供共享读写 API、`myproxyctl` 命令，以及设置页入口。

## 范围

- `strategy.rs`：`export_to` / `import_from`（导入前备份）+ 临时目录单测
- `myproxyctl`：`export [path]`、`import <path>`，更新 `capabilities`
- 设置页「配置」面板：系统文件面板 + 导入确认
- AGENTS / agent skill / README 各加一两句

## 非目标

- 导出 catalog / runtime / 节点缓存
- 选择性合并、加密、多配置档案
- 改 schema 版本号
- 自动 `connect`
- 提交或发 PR（除非用户另说）

## 关键约束

- 只改 worktree `/Users/harry/project/myproxy/.worktrees/feat-strategy-import-export`
- 整份替换，不做端口/开关合并
- GUI 导出当前内存 Strategy；CLI 先 `load`
- CLI 不自动 apply；GUI 在 wanted 时 start_apply
- 不打印订阅 URL
- 不新增 `rfd` 依赖

## 修改路径

1. `src/strategy.rs`：共享导出/导入 API + 单测
2. `src/bin/myproxyctl.rs`：export / import + capabilities
3. `src/file_dialog.rs` + `src/ui.rs` 设置页
4. AGENTS / skill / README

## 验证方式

- 针对性 `cargo test`（strategy）
- `cargo fmt`
- `cargo check --bins --tests`
- `myproxyctl --json capabilities` 含 export / import

## 验收标准

- `export_to` 写出的文件能被 `load_from` 读回
- 旧 schema 导入后 migrate 到当前 schema
- 坏 JSON 不改本机文件
- 导入前备份存在且内容是导入前策略
- capabilities 含 export / import
- 设置页可导出到下载文件夹、导入前确认

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
| 整份替换 strategy.json | 与完整备份/恢复一致 |
| 导入前写 bak-import 时间戳 | 失败可回退，不覆盖无备份 |
| CLI 不 apply | 与现有 Agent 约定一致 |
| osascript 文件面板 | 不新增 rfd，macOS 原生选择器 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| worktree 无 rustfmt | 1 | rustup component add rustfmt |
| `message` 被 Fn 闭包移出 | 1 | `message.clone()` |
