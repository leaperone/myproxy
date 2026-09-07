# 任务计划：ne-gfw-via

- 任务 ID：`ne-gfw-via-2026-09-07_17-05-17`
- 创建时间：`2026-09-07_17-05-17`

## 目标

系统接管（NE）拦到的进程或域名，规则走向可选：直连、节点组，或按 GFWList 判断（命中走指定组，未命中直连）。

## 范围

- 规则 `via` 增加 `gfw:<组名>`；规则编辑器走向菜单提供「GFWList → 组」
- NE 快照编进程 + 域名/后缀/关键字/网段；`gfw:` 编成「GFW 命中 → 组 SOCKS，其余直连」
- 加载已缓存的 Loyalsoldier `ruleset/gfw.yaml`（缺文件则尝试拉取）

## 非目标

- 不改 Mixed 四档 / 不拆内置 mihomo
- 不把全局「分流」预设改成按规则；页面级 GFWList 预设保留
- 不实现多上游 / 外部 host:port

## 关键约束

- 系统接管 `extension_mode=rule` 时才编用户规则进 NE
- `gfw:` 在 mihomo YAML 里展开为组名（Mixed 近似为整条走该组）；GFW 拆分只保证 NE 路径
- 主仓 main 上有未提交的 Mixed debug，本任务只写 worktree

## 修改路径

- `src/gfw.rs`、`src/compile.rs`（`via_target`）、`src/network_extension.rs`、`src/ui.rs`
- `macos/NetworkHost/HostBridge.swift`
- `AGENTS.md` 一句 via 语义

## 验证方式

- `cargo test --offline`：via_target / gfw 解析 / inbound_plan 含 dest 与 gfw
- 人工：规则选 GFWList→Default，开系统接管后进程访问列表内/外域名

## 验收标准

- 走向可选直连、组、`GFWList → 组`
- 进程 + `gfw:组`：列表内进该组 SOCKS，其余直连，不进 profile-rules
- 域名规则同样编进 NE
- 旧 `via=DIRECT|组名` 行为不变

## 未确认事项

无。GFW 未命中固定直连（不做成「未命中走另一组」）。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| `via=gfw:<组>` | 不 bump schema，一条字符串就能进编辑器和 CLI |
| GFW 域名单次放进 EnableRequest | HostBridge 按规则展开，避免每个进程复制一份到 Rust 结构 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| | 1 | |
