# 调研与结论：修复应用策略卡顿与核心重连

## 已确认根因

- `AppView::start_apply` 在后台等待 `Supervisor::apply`；`Supervisor::apply` 同步执行 `catalog::refresh`，订阅按顺序刷新。
- `catalog::fetch_http_body` 先运行 curl IPv4（最多 45 秒），失败后再用 ureq（最多 45 秒），单订阅最坏约 90 秒，多个订阅累计等待，UI 只显示“正在应用策略…”。失败虽会复用缓存，但要等请求结束。
- `Supervisor` 只记录 `running_tun`/`running_se`，不记录运行 Mixed 端口。端口修改后仍对新端口调用 `controller::reload`，旧核心继续监听旧端口，造成 controller/probe 不一致。
- `disconnect_inner` 对 pid 文件中的跨进程核心发送 SIGTERM 后立即返回；新核心可能与旧核心短暂争抢端口。
- `network_extension::disable_async` 和 `enable_async` 各自启动线程，没有顺序令牌；重连时 stop 线程可能晚于 enable 线程完成，导致扩展最终停用。
- 核心已退出但 wanted 文件仍存在时，`Supervisor::apply` 只编译并返回成功，不会主动重连，UI 会显示应用成功但 controller 仍不可达。

## 证据

- macmini `myproxy.log`：保存后出现 `strategy saved` → `supervisor apply`，随后订阅失败等待；应用启动时仍显示 `system extension stopped`。
- `src/catalog.rs` 的 curl/ureq 双 45 秒超时和 `src/supervisor.rs` 的 apply/reload/端口跟踪逻辑与上述日志一致。

## 设计决定

- 先保留缓存复用语义，收紧单次网络请求和回退等待，不改变订阅失败后的结果。
- 以运行端口变化作为重连条件，并在停机后等待旧端口释放。
- 用带序号的网络扩展操作线程，旧操作完成后检查序号，避免过时 enable/disable 覆盖最新意图。
- apply 统一检查 wanted/running 状态，核心缺失时用已生成目录直接重连。
