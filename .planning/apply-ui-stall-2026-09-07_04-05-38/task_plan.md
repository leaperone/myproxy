# 任务计划：修复保存/重载卡顿与 UI 状态滞后

## 目标
定位并修复策略保存、重新加载、应用配置时的长时间阻塞和 UI 状态不及时，确保操作串行、状态可见、核心未就绪时不会误报成功。

## 范围
- `src/ui.rs` 保存/应用状态、连接与连接列表轮询。
- `src/supervisor.rs` apply/connect/health reconnect 调用链。
- `src/catalog.rs` 订阅刷新与缓存策略（仅在证据要求时修改）。
- 相关测试和日志。

## 非目标
- 不改变节点组、规则语义和默认路由。
- 不重做 UI 视觉设计。
- 不修改用户本机或 macmini 的策略内容作为代码交付。

## 约束
- 基于最新 `origin/main`，使用独立 worktree。
- 保留现有策略缓存和系统接管行为。
- 不记录或输出订阅 URL、密钥等敏感数据。

## 验收标准
- 普通保存/应用不重复刷新未变化的订阅。
- apply/connect/health reconnect 不并发、不重复启动核心。
- 核心或 controller 未就绪时 UI 显示明确处理中/错误，不长时间停留在旧状态。
- 现有测试及新增针对性测试通过。
- preflight 五门通过并完成 squash merge。

## 验证方式
- cargo check/build/test、git diff --check。
- 针对 Supervisor/Catalog/UI 状态的单元测试或可观测日志验证。
- preflight 与 CI。
