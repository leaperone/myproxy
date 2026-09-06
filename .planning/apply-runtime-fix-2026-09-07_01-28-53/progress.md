# 执行进度：修复应用策略卡顿与核心重连

## 已完成
- 完成 macmini 日志和调用链诊断，确认订阅刷新超时、Mixed 端口 reload 错用、跨进程停机等待和系统扩展异步竞态。
- `v0.0.5` Prod 发布已核验，当前修复目标为后续 patch。

## 已完成
- 在独立 worktree 实现订阅快速失败、缓存目录复用、Mixed 端口变化重连、核心缺失主动重连、跨进程停机等待和系统扩展操作串行化。
- UI 仅在订阅/过滤器变化或目录为空时刷新订阅，其余策略保存直接使用缓存目录。
- 追加清理过时 enable 标记、校验 PID 与 Mixed 端口、清除退出核心的 pid 文件，并限制失败订阅缓存只在 URL/过滤器一致时复用。

## 验证结果
| 检查 | 结果 | 状态 |
|---|---|---|
| `cargo check --quiet` | 通过；仅既有 Objective-C `cargo-clippy` cfg 警告 | 通过 |
| `cargo test --quiet` | 13 个测试通过 | 通过 |
| `cargo build --quiet` | 通过；仅既有 Objective-C `cargo-clippy` cfg 警告 | 通过 |
| `git diff --check` | 无空白错误 | 通过 |
| macmini 保存配置实测 | 待修复后执行 | 待执行 |

## 错误与恢复
暂无。
