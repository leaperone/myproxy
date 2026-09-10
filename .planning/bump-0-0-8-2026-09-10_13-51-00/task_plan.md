# 任务计划：bump-0-0-8

- 任务 ID：`bump-0-0-8-2026-09-10_13-51-00`
- 创建时间：`2026-09-10_13-51-00`

## 目标

把 main 开发基线从已发布 Prod `v0.0.7` 前移到 `0.0.8`，以便 Nightly 版本高于最新 Prod。

## 范围

- `Cargo.toml`、`Cargo.lock` 中 myproxy 包版本
- 两个 `packaging/macos/**/Info.plist` 版本字段
- `.preflight.toml` 为版本文件补 scope，避免预检 unverified

## 非目标

- 不改功能代码
- 不打 Prod tag

## 关键约束

- 正式目标必须高于最新 Prod `v0.0.7`
- 不覆盖已发布 tag

## 修改路径

- `Cargo.toml`、`Cargo.lock`
- `packaging/macos/Info.plist`
- `packaging/macos/NetworkExtension/Info.plist`
- `.preflight.toml`

## 验证方式

- 核对四处版本均为 `0.0.8`
- preflight 五门后 squash merge

## 验收标准

- `origin/main` 目标版本为 `0.0.8`
- 最新 Prod 仍为 `v0.0.7`

## 未确认事项

无。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 先升 0.0.8 再发 Nightly | Nightly 目标必须高于已发布 Prod |

## 错误与处理

无。
