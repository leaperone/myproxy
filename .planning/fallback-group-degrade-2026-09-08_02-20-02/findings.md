# 调研与结论：fallback-group-degrade

- 任务 ID：`fallback-group-degrade-2026-09-08_02-20-02`
- 创建时间：`2026-09-08_02-20-02`

## 需求事实

- 用户：节点组内应循环自动降级到**另一个节点**，不是 DIRECT。
- 现场：Default 是 fallback，卡在超时 Kitty 节点；业务拨号失败不会切 `now`。

## 真实调用链

- `compile.rs` 把 fallback / url-test 编成 mihomo 组，原先只有 `url=gstatic` + `interval=300`。
- mihomo 用定时探测标记 alive，再按列表取第一个活着的成员。
- 空组仍插入 DIRECT，这是无成员占位，不是降级。

## 调研结论

- 300s 间隔 + 默认 lazy / 较高 max-failed-times，死节点可以一直当 `now`。
- 应缩短探测、关闭 lazy、两次失败即标记 down，让组走到下一个成员。

## 技术决策

| 决策 | 证据 |
|---|---|
| interval 30 / timeout 3000 / lazy false / max-failed-times 2 | 编译字段直接决定 mihomo 何时换人 |
| 非空组断言不含 DIRECT | 用户要求降级只换节点 |

## 风险与边界

- 仍不是「这一笔 TCP 失败立刻换节点重拨」；下一笔连接才会用新的 now。
- 全员探测失败时 mihomo 仍可能停在最后一个 now。

## 参考指针

- `src/compile.rs` `insert_auto_group_probe`
