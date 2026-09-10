# Findings

- 本机 38.1 上 `1579`（周二）同时占用 `7891` 和 `1053`；`38041` 是 15:18 新核，pid 文件指向它，但没有这两个端口。
- `scutil --dns` 第一解析器仍是局域网 `192.168.0.1`。黄字来自 Host 对 DNS 报告的 `validate`（心跳/activation 不符）。
- `start_runtime` 只检查「本机是否有人听 7891」，不检查是不是刚 spawn 的 child。
- `is_running()` 看 pid 文件，新 GUI 进程会 `reload` 而不是回收兄弟进程。07:18 日志有 capture stop，说明走了 disconnect，但就绪仍可能认孤儿的 Mixed。
- `04:24:03Z stopDNSProxy: permission denied (NEDNSProxyErrorDomain 1)`。
- 旧扩展 30.1/32.1/36.1 仍 `waiting to uninstall on reboot`。
