# 任务计划：gfwlist-routing

- 任务 ID：`gfwlist-routing-2026-09-07_12-18-45`
- 创建时间：`2026-09-07_12-18-45`

## 目标

规则页用三档分流预设收编未匹配与 Loyalsoldier GFWList，整包装卸，不拆进 `rule_sets`。

## 范围

- schema 7：`routing_profile` = allowlist | gfwlist | group
- compile：`rule-providers` + `RULE-SET,gfw`；`mihomo -d data_dir`
- 规则页三档；设置页去掉未匹配；ctl `routing`；文档

## 非目标

- 不解析官方 gfwlist Base64，不接自定义 URL / 多份列表
- 不改 Mixed / 系统接管四档

## 关键约束

- 用户规则在 RULE-SET 前；GFW 的 MATCH 必须 DIRECT
- 不把 GFW URL 写入 strategy，不在日志打印完整 URL

## 修改路径

- `src/strategy.rs`、`src/compile.rs`、`src/supervisor.rs`、`src/paths.rs`
- `src/ui.rs`、`src/bin/myproxyctl.rs`、`src/lib.rs`
- `AGENTS.md`、`CLAUDE.md`、agent skill

## 验证方式

- compile / migrate 单测
- cargo fmt / check / test / build
- 有 mihomo 时对 GFW 档跑 `-t`

## 验收标准

- 三档互斥；卸 GFW 不改用户规则表
- 旧 unmatched_via 非 DIRECT 迁成 group

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
| 预设即安装状态，不另存 provider 列表 | 换走即卸载 |
| jsdelivr gfw.txt + 内核拉取 | 不自己解析 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| | 1 | |
