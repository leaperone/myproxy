# 调研与结论：resolve-via-group-alias

- 任务 ID：`resolve-via-group-alias-2026-09-06_21-23-11`
- 创建时间：`2026-09-06_21-23-11`

## 需求事实

- 本机连接失败：`supervisor runtime YAML failed mihomo -t`
- `mihomo -t`：`rules[18] [DOMAIN-KEYWORD,github,default] error: proxy [default] not found`
- 策略组名为 `Default`，多条规则 `via` 为 `default`；NE listener 也写了 `proxy: default`
- `MATCH` 已写成 `Default`，因为 `default_group` 在找不到精确 `PROXY` 时用第一组

## 真实调用链

- `Supervisor::connect` → `compile::compile` → 每条 `rule_set.via` 走 `via_target`
- `network_extension::inbound_plan` 同样用 `via_target` 生成 `proxy:` listener
- `via_target` 只精确匹配组名；`default`/`reject`/`direct` 里只有后两者是别名
- UI/tray/`ensure_telegram_routing` 已用 `eq_ignore_ascii_case("default") || name == "PROXY"`

## 调研结论

- 新策略默认组名是 `PROXY`，`Default` 是用户改名/导入结果
- 仓库没有 compile/via 测试
- 最小修复：`via_target` 忽略大小写找组并返回保存名；找不到时把 `default`/`proxy` 落到 `default_group`
- `default_group` 应同时认忽略大小写的 `default`

## 技术决策

| 决策 | 证据 |
|---|---|
| 返回策略保存的组名 | mihomo 组名区分大小写；`Default` != `default` |
| `default`/`PROXY` 互为默认组别名 | `src/ui.rs`、`src/tray.rs`、`src/strategy.rs` 已这样认 |
| 不改 strategy.json | 编译层对齐即可让现有配置启动 |

## 风险与边界

- 若同时存在 `Default` 与 `PROXY`，精确/忽略大小写命中优先于别名；`default` 会命中 `Default`
- 主仓 dirty 文件（`CaptureRuleEngine.swift`、`DEFAULT_DIRECT_RULES`、release skill）不属于本任务
- 已安装的 `/Applications/myproxy.app` 仍是旧编译器；GUI 再点连接会用旧逻辑重写 YAML。需要新包才对安装版生效
- 从 cargo 启动的 `connect` 已拉起 mixed-port，但提示 System Extension 需要 bundled `.app`

## 参考指针

- `src/compile.rs`：`via_target`、`default_group`
- `src/network_extension.rs:81`
- 失败 runtime：`~/Library/Application Support/myproxy/runtime.yaml`
