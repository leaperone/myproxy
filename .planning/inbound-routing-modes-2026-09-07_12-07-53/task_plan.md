# 任务计划：inbound-routing-modes

- 任务 ID：`inbound-routing-modes-2026-09-07_12-07-53`
- 创建时间：`2026-09-07_12-07-53`

## 目标

给 Mixed 与 System Extension 各加独立四档入站路由（rule / proxy / global / direct），默认 rule。TUN 不加第三旋钮。SE 开关与 TUN 互斥保持不变。

## 范围

- strategy schema 5 → 6：`mixed_mode` / `extension_mode`，serde 默认 rule
- compile：去掉顶层 `mixed-port`，始终写 `listeners` mixed；SE 打开时与 NE SOCKS 同数组；按档写 `proxy`；SE 非 rule 不发分组 SOCKS
- 设置页两组四档、总览副标题、myproxyctl、AGENTS/CLAUDE/skill
- compile / migrate 单测 + cargo fmt/check/test/build

## 非目标

- TUN 独立模式
- 改 SE/TUN 互斥或默认开关
- 改 unmatched_via 语义（仍只在 rule 档生效，设置页保留）
- 提交 / 发 PR（除非用户另说）

## 关键约束

- 只改 worktree `/Users/harry/project/myproxy/.worktrees/feat-inbound-routing-modes`
- 不改 Cursor plan 文件
- 旧配置缺字段保持 rule
- 不打印订阅 URL
- UI 中文标签：按规则 / 默认组 / 全局 / 直连
- 已连接（wanted）时 persist_and_apply，否则只 persist

## 修改路径

1. `src/strategy.rs`：`InboundMode` + 两字段 + schema 6
2. `src/compile.rs`：listeners / `inbound_proxy` / 去掉 mixed-port
3. `src/network_extension.rs`：非 rule 时空 `process_rules` / `group_ports`
4. `src/ui.rs`：设置页四档 + 总览副标题
5. `src/bin/myproxyctl.rs`：mixed-mode / extension-mode + status + capabilities
6. `AGENTS.md`、`.cursor/AGENTS.md`、两份 agent skill
7. 单测覆盖四档、SE rule/非 rule、无 mixed-port、缺字段默认

## 验证方式

- `cargo fmt`
- 针对性 `cargo test`（compile / strategy）
- `cargo check`
- `cargo test`
- `cargo build`

## 验收标准

- 旧 JSON 无字段反序列化为 rule，schema migrate 到 6
- Mixed 四档对应 listener `proxy`：无 / default_group / GLOBAL / DIRECT
- SE rule 仍有分组 SOCKS；非 rule 仅默认 SOCKS 且带 proxy
- 顶层无 `mixed-port`；SE 开时 mixed 与 NE SOCKS 同在 `listeners`
- 设置页两组四档可选（SE 关着也能选）；wanted 时 apply
- myproxyctl 能读写两档；capabilities / status 包含
- 文档一句说明两档独立且默认 rule

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
| `InboundMode` 枚举 + serde lowercase | 与 JSON 字面量对齐，缺字段走 Default=rule |
| 抽出 `compile_root` 供单测 | `compile` 会写 runtime.yaml，测试不碰用户数据目录 |
| SE 非 rule 时 inbound_plan 直接空规则 | 与 compile 不发分组 SOCKS 一致，捕获全走默认 SOCKS |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| rustfmt 1.98.1 组件下载失败 | 2 | 用 stable rustfmt；无关文件已回退 |
| 并行 GFWList 改同一 worktree | 1 | 保留其分流改动，补齐 inbound helper |
