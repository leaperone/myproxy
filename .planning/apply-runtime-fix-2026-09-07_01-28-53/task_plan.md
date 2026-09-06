# 任务计划：修复应用策略卡顿与核心重连

## 目标
修复 nightly 中保存配置后长时间停留“正在应用策略…”以及修改 Mixed 端口后偶发无法连接核心的问题。

## 范围
- 缩短订阅 HTTP 刷新失败路径的总等待时间，避免一次应用因重复网络回退卡住数分钟。
- 记录运行中核心的 Mixed 端口；端口变化时重启核心而不是向新端口发送旧进程 reload。
- 串行化系统扩展停用与启用，避免应用策略时异步操作乱序。
- 应用策略时若 wanted 仍为真但核心已退出，主动重连，不等待健康轮询才恢复。
- 为上述决策补充可验证的单元测试和日志/状态反馈。

## 非目标
- 不改变策略匹配语义、Telegram 规则、节点选择或 mihomo 配置格式。
- 不重构订阅解析器或网络扩展实现，只修复本次确认的竞态和超时路径。

## 约束
- 遵守仓库 AGENTS.md；中文交付，不写时间估算。
- 通过独立 worktree 开发；完成 commit、push、PR、preflight。
- 修复完成后将正式版本推进到最新 Prod 基线之后的 patch 版本。

## 修改路径
- `src/catalog.rs`：订阅请求超时和回退策略。
- `src/supervisor.rs`：运行端口跟踪、端口变化重启、跨进程停机等待。
- `src/network_extension.rs`：停用/启用操作串行化。
- 相关 planning 与测试。

## 验证方式
- `cargo check --quiet`
- `cargo test --quiet`
- `cargo build --quiet`
- `git diff --check`
- planning `check-complete.sh`
- preflight 五门闸及 CI
- macmini 安装后检查版本、controller 和保存配置流程日志。

## 验收标准
- 订阅请求失败时单次应用不会因重复 HTTP 回退长时间无响应，并继续使用已有缓存。
- Mixed 端口变更后核心监听新端口，controller 与 UI 状态一致。
- 系统接管模式切换不会因 stop/enable 异步乱序导致扩展最终停用。
- 现有策略行为和测试保持通过。

## 未确认事项
- macOS GUI 中的 VoiceOver/窗口现场验证不在当前命令行环境内。
