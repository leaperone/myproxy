# 任务计划：onboarding-guidance

- 任务 ID：`onboarding-guidance-2026-09-11_00-22-43`
- 创建时间：`2026-09-11_00-22-43`

## 目标

新用户在 App 内能走完完整配置：订阅 → 节点 → 入站说明 → 连接。页内空状态和介绍符合仓库 AGENTS 与 better-writing。

## 范围

- 总览配置检查与下一步主按钮
- 把 Mixed / 系统接管 / TUN 何时生效写进 App
- CLI 首次弹窗不再挡第一幕
- 系统接管写完整系统设置路径，并提供打开系统设置
- 空状态带下一步按钮；统一「显示直连」「未命中直连」
- README 页面列表对齐侧栏

## 非目标

- 不做多步 wizard / coach mark
- 不改路由编译或系统接管捕获语义
- 不发 Nightly

## 关键约束

- 不打印订阅 URL 或 NE 凭证
- 无订阅时仍可连接（不锁死），但主按钮指向下一步并说明空组 REJECT
- 设计和规范只在 AGENTS.md 引用，不复制

## 修改路径

- `src/setup.rs`
- `src/ui.rs`
- `src/onboard.rs`
- `src/network_extension.rs`
- `src/strategy.rs`
- `src/lib.rs`
- `AGENTS.md`
- `README.md`

## 验证方式

- `cargo test --lib setup::`
- `cargo test --lib strategy::`（label）
- 对照总览 / 空状态 / 设置文案的源码检查

## 验收标准

- 无订阅时总览主按钮是「添加订阅」，不是只推「连接」
- 总览写清三条入站及 Mixed 地址
- 启动不再自动弹出安装 CLI
- 设置页能打开登录项与扩展，并写出完整路径
- 「显示直连」「未命中直连」用词一致

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
| 总览用检查清单，不锁死连接 | Apple Agency：仍可连，但下一步更显眼 |
| CLI 只留在设置 | 第一幕不应推销 Agent |

## 错误与处理

无。
