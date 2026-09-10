# 任务计划：ne-mihomo-routing

- 任务 ID：`ne-mihomo-routing-2026-09-10_13-21-05`
- 创建时间：`2026-09-10_13-21-05`

## 目标

系统接管只认进程并送到 mihomo 入口；域名 / GFW / MATCH 只在 mihomo 评。
进程 `gfw:<组>` 送到专用 GFW 入口（命中走该组，未命中直连）。

## 范围

- NE `inbound_plan` 不再编 dest / 不再下发 GFW 域名列表
- 进程钉组仍走组 SOCKS；进程 `gfw:` 走无 `proxy:` 的 GFW 入口
- mihomo `sub-rules` + `IN-NAME` 评 `RULE-SET,gfw`
- 域名 `gfw:` 编成 `AND` + `RULE-SET`
- 更新 AGENTS.md 分流边界

## 非目标

- 不改规则页 UI / schema
- 不把进程身份交给 mihomo 评（接管后进程名是扩展）
- 不发布 Nightly（未授权）

## 关键约束

- Apple NE 偏好 ≤ 512KB，不得再展开 GFWList
- GFW 入口语义：命中 → 组，未命中 → DIRECT
- 规则 JSON 仍用 `myproxyctl`，不直接改用户机器 strategy

## 修改路径

- `src/network_extension.rs`、`src/compile.rs`
- `macos/NetworkHost/HostBridge.swift`
- `AGENTS.md`（CLAUDE.md 保持相对链接）

## 验证方式

- `cargo test --lib compile::` 与 `network_extension::`
- 编译测：`sub-rules`、listener `rule`、域名 AND+RULE-SET

## 验收标准

- Safari `gfw:Default` 不再把 4400 域名编进 NE snapshot
- 进程 → 组仍分配组 SOCKS
- 域名规则只出现在 mihomo YAML，由 profile-rules 入口评
- 有 `gfw:` 或分流=GFWList 时写入 rule-provider

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
| GFW 用 listener `rule` + sub-rules，并加 IN-NAME 兜底 | 名单只在 mihomo；旧核忽略 `rule` 时 IN-NAME 仍拆 |
| NE 不再编 dest | 目的地由 profile-rules 评，避开 512KB |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| worktree 误停在 feat/se-ai-proxy-route | 1 | checkout -B origin/main |

## 完成定义对照

- 实现与验收标准对齐：是
- 验证已执行：`cargo test --lib compile::` 20 passed；`network_extension::` 4 passed
- 未完成项：无
