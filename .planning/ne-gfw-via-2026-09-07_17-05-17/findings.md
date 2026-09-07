# 调研与结论：ne-gfw-via

- 任务 ID：`ne-gfw-via-2026-09-07_17-05-17`
- 创建时间：`2026-09-07_17-05-17`

## 需求事实

- 用户要在 NE 接管的进程或域名上自选：直连、节点组、或 GFWList 判断直连/某组

## 真实调用链

- 规则页 `via` → `compile::via_target` → mihomo YAML；SE 开时 `app` 不写 PROCESS-NAME
- `network_extension::inbound_plan` 只收集 `app` → HostBridge `captureSnapshot` 进程规则 + 兜底 `profileRules`
- CaptureRuleEngine 已支持 dest host/suffix/pattern/cidr，低 priority 先命中
- 缓存列表：`~/Library/Application Support/myproxy/ruleset/gfw.yaml`，payload `+.domain.com`

## 调研结论

- 进程 GFW：高优先 source=进程 dest=GFW → 组；低优先 source=进程 dest 空 → 直连
- 域名 GFW：dest = GFW ∩ 用户 matcher → 组；用户 dest → 直连（更低优先）

## 技术决策

| 决策 | 证据 |
|---|---|
| 不新增 CaptureAction | 两条 CaptureRule 即可 |
| Mixed 展开为组名 | YAML 不能按流查 GFW 子集；NE 才是拆分点 |

## 风险与边界

- GFW 约 4k 域；多个进程共用 `gfw:` 时快照会变大
- 无缓存且拉取失败时，`gfw:` 进程规则退化成全直连

## 参考指针

- `src/network_extension.rs` inbound_plan
- `macos/NetworkHost/HostBridge.swift` captureSnapshot
- `src/compile.rs` GFW_LIST_URL / via_target
