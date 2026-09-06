# 调研结论

- 本机日志显示一次 apply 会刷新 3 个订阅并编译；短时间内又出现重复 apply，首次 apply 到核心监听约 23 秒。
- connect 期间 UI connections/proxies 轮询多次解析失败；health reconnect 会再次完整刷新订阅并切换 System Extension。
- 当前主线已包含缓存 apply、端口重连和 stale PID 修复，但实际 UI 仍需核对调用方是否绕过缓存或重复触发。
- macmini 与本机策略规则修改后运行时配置包含新规则；本轮代码任务不改变这些策略。

## 复核后的根因

- `OPERATION` 的命名 guard 实际会保持到函数作用域结束，不能把 NLL 提前释放作为已证实根因；真正缺口是没有跨进程锁，且启动后的健康探测窗口过短。
- `start_with_catalog` 等待核心端口约 2–8 秒，但健康检查可能在 System Extension 尚未稳定时进入恢复流程，重复重连并再次刷新订阅。
- 订阅失败时 curl 后再用第二个 HTTP 客户端会重复支付超时；改为单一路径后能把失败上限降下来并继续使用已有缓存。
- UI busy 状态没有耗时反馈；订阅失败复用缓存也没有在状态文案显示。
