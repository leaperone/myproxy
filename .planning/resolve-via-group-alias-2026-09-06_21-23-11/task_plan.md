# 任务计划：resolve-via-group-alias

- 任务 ID：`resolve-via-group-alias-2026-09-06_21-23-11`
- 创建时间：`2026-09-06_21-23-11`

## 目标

编译 runtime YAML 时，把规则 `via` 的 `default` / `PROXY`（忽略大小写）解析成策略里真实存在的组名，避免 `mihomo -t` 因 `proxy [default] not found` 拒绝启动。

## 范围

- 修正 `via_target`：组名大小写不敏感，并返回策略里保存的名字
- 把 `default` / `proxy` 当作默认组别名（`PROXY` 或忽略大小写的 `Default`）
- `default_group` 与 UI/tray 对齐：认 `PROXY` 或忽略大小写的 `default`
- 为 `via_target` 增加单元测试

## 非目标

- 不改用户 `strategy.json` 里的组名或规则 `via`
- 不改 UI 存储、不强制把组改回 `PROXY`
- 不处理订阅节点、System Extension 捕获引擎、默认直连规则
- 不把主仓上已有无关改动带进本任务

## 关键约束

- 主 checkout 是 `main`，且有无关 dirty 文件，必须在独立 worktree 写代码
- 组名对 mihomo 区分大小写，编译输出必须用策略里的真实名字
- 不记录订阅 URL

## 修改路径

- `src/compile.rs`：`via_target`、`default_group`、单元测试

## 验证方式

- `cargo test --lib`：`via="default"` + 组 `Default` 解析为 `Default`；`via="PROXY"` + 组 `Default` 也解析为 `Default`；精确组名与 `DIRECT`/`REJECT` 不变
- 用修复后的 `myproxyctl apply` / `connect` 对当前策略编译，再跑 `mihomo -t`

## 验收标准

- `via_target("default", …)` 在组名为 `Default` 时返回 `Default`
- `via_target("PROXY", …)` 在只有 `Default` 组时返回 `Default`
- `via_target("AI Proxy", …)` 精确/忽略大小写命中后返回真实组名
- `DIRECT` / `REJECT` 行为不变
- 单元测试通过
- 本机 `runtime.yaml` 的 GitHub 等规则目标为 `Default`，`mihomo -t` 成功

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
| 编译时解析别名，不改用户策略 | 用户组名是 `Default`，规则 via 是 `default`；改编译即可让 mihomo 通过 |
| 同时认 `default`/`PROXY` 别名 | UI/tray/strategy 已把二者当同一默认组 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| 无 | 1 | |
