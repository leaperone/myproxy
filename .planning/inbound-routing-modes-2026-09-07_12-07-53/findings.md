# 调研与结论：inbound-routing-modes

- 任务 ID：`inbound-routing-modes-2026-09-07_12-07-53`
- 创建时间：`2026-09-07_12-07-53`

## 需求事实

- Mixed 与 SE 各四档：rule（现有 rule_sets + MATCH）、proxy（default_group）、global（GLOBAL）、direct（DIRECT）
- TUN 不加旋钮；SE 开关与 TUN 互斥不变
- unmatched_via 只在 rule 档生效，设置页保留
- 旧配置缺字段 = rule

## 真实调用链

- `Strategy` serde 反序列化 → `migrate()` 升 schema → `compile()` 写 runtime.yaml
- 当前 `compile` 写顶层 `mixed-port`；SE 开时另写 `listeners` NE SOCKS（默认一对 + 每组一对），`inbound_plan` 从 app matcher 收集 process_rules / group_ports
- 设置：`system_extension_panel` 开关走 `set_system_extension`（wanted 则 persist_and_apply）；Mixed 端口只 persist
- `myproxyctl`：`port` / `tun` / `extension` / `unmatched`；`capabilities` 列出命令；`status` 打 JSON + 一行文本
- `AGENTS.md` 与 `.cursor/AGENTS.md` 正文相同；`CLAUDE.md` 是 AGENTS.md 的符号链接
- 两份 agent skill 文档化 ctl 命令

## 调研结论

- schema 只需 bump 到 6 并加默认字段；migrate 无结构折叠
- Mixed 必须改 listener，否则无法按入站写 `proxy`（顶层 mixed-port 不能绑 outbound）
- SE 非 rule 必须清空 process_rules / group_ports，否则 NE 仍按进程钉到分组 SOCKS
- 总览 `page_title` 副标题目前只谈 SE 开关，需改成「Mixed 按规则 · 接管 全局」这类双档文案

## 技术决策

| 决策 | 证据 |
|---|---|
| `inbound_proxy(mode, strategy) -> Option<String>` | 用户指定：rule=None，proxy=default_group()，global=GLOBAL，direct=DIRECT |
| 始终 emit mixed listener `myproxy-mixed` | 去掉顶层 mixed-port 后 Mixed 入口必须在 listeners |
| SE 开时 mixed + NE SOCKS 同一 `listeners` | 当前 `insert_network_extension_listeners` 独占 listeners，需合并 |

## 风险与边界

- `inbound_plan` 用进程级 SESSION；单测并发时 group_ports 可能残留，但非 rule 请求不输出残留端口
- SE 开时现有逻辑仍省略 PROCESS-NAME（进程交给 NE）；SE 非 rule + Mixed rule 时 Mixed 侧进程规则仍不进 YAML（既有耦合，本次不扩范围）
- `compile()` 写用户 data_dir；单测走 `compile_root` 避免污染
- 同 worktree 并行 GFWList：未匹配面板从设置挪到规则页「分流」；`unmatched_target` 现受 `routing_profile` 约束

## 参考指针

- `src/strategy.rs` STRATEGY_SCHEMA / Strategy 字段 / migrate
- `src/compile.rs` compile / insert_network_extension_listeners / default_group
- `src/network_extension.rs` inbound_plan
- `src/ui.rs` settings / system_extension_panel / overview page_title
- `src/bin/myproxyctl.rs` Commands / Capabilities / Status
