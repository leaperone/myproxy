本次补丁修复 Xray 系统接管下的 DNS 递归和节点端点解析问题，并减少未使用节点的后台探测。

- 上游 DNS 地址在 Xray 自身启动 DNS/SOCKS 旁路时直接绕过 App Admission 和私有 SOCKS，避免 DNS → SOCKS → DNS 循环。
- 节点服务器域名在启动 Xray 前解析为连接地址，同时保留 TLS SNI 和 WebSocket Host。
- 自动健康探测只检查实际被节点组引用的节点，未使用的订阅节点不会消耗 DNS 和探测资源。
- 现有规则、端口、节点组选择和订阅配置保持不变。

安装包使用 Developer ID 签名并经过 Apple 公证，适用于 Apple Silicon Mac，macOS 14 或更新版本。
