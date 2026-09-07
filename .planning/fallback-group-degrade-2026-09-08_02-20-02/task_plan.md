# 任务计划：fallback-group-degrade

- 任务 ID：`fallback-group-degrade-2026-09-08_02-20-02`
- 创建时间：`2026-09-08_02-20-02`

## 目标

让 fallback / url-test 节点组在当前节点探测失败时，按成员列表自动切到下一个节点，而不是停在已超时的 `now`，也不把「降级」做成 DIRECT。

## 范围

- 收紧 `compile.rs` 里自动组的健康探测（interval / timeout / lazy / max-failed-times）。
- 单测断言编译结果。
- AGENTS.md 补一句产品行为。

## 非目标

- 不把 DIRECT 加进非空组末尾。
- 不实现同一条 TCP 连接失败后原地换节点重拨。
- 不改 Mixed / 系统接管模式。
- 不改 Sparkle。

## 关键约束

- 空组仍可编译为仅 DIRECT（无成员，不是降级）。
- 探测 URL 保持 gstatic generate_204。

## 修改路径

- `src/compile.rs`
- `AGENTS.md`

## 验证方式

- `cargo test --lib compile::`

## 验收标准

- fallback / url-test 编译出更积极的探测，失败后让下一个成员成为 alive。
- 非空组不把 DIRECT 当作降级成员。
- 单测覆盖这些字段。

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
| 30s 探测、3s 超时、两次失败、关闭 lazy | 300s + 默认 lazy 会让死节点长时间占着 now |
| 不把 DIRECT 当组内降级 | 用户明确降级只换节点 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| 空 catalog 会把 all_nodes 组编成 DIRECT | 1 | 单测放入一个节点 |
