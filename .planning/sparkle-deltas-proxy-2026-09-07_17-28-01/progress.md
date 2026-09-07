# 执行进度：sparkle-deltas-proxy

- 任务 ID：`sparkle-deltas-proxy-2026-09-07_17-28-01`
- 创建时间：`2026-09-07_17-28-01`
- 当前状态：`in_progress`

## 已完成

- Nightly / Prod 同通道旧 zip 选择
- Sparkle session 注入 Mixed HTTP 代理
- 设置页与文档

## 进行中

- commit / PR / preflight

## 修改文件

- `scripts/release-macos.sh`、`scripts/sparkle_previous_tags.py`
- `packaging/macos/sparkle_bridge.m`、`src/sparkle.rs`、`src/supervisor.rs`、`src/ui.rs`
- `README.md`、`AGENTS.md`

## 验证结果

| 检查 | 结果 | 状态 |
|---|---|---|
| `sparkle_previous_tags.py --self-test` | ok | 通过 |
| 对现网 release 选 Nightly 旧 tag | 跳过 `nightly`，取最近 6 个 Nightly | 通过 |
| 对现网 release 选 Prod 旧 tag | v0.0.6 … v0.0.1 | 通过 |
| `cargo test --lib` | 35 passed | 通过 |
| `cargo check --bins --features sparkle` | ok | 通过 |

## 错误与恢复

| 错误 | 尝试 | 解决方式 |
|---|---:|---|
| worktree 落到 `feat/ne-gfw-via` | 1 | checkout 回 `feat/sparkle-deltas-proxy` |
| gh 无代理连不上 GitHub | 1 | `https_proxy=http://127.0.0.1:7891` |
