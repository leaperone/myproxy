Xray 是 MyProxy 与正式版、Nightly 并列的发布通道。版本使用项目的基础版本号，加上 `xray.日期.构建号.重试号`；应用名称保持 MyProxy。

- MyProxy 管理规则、节点组、地区优先级、自动切换和连接记录，Xray-core 负责代理连接。
- HTTP 和 SOCKS5 共用 40808，支持网络扩展接管和按应用分流。
- 包含升级后恢复连接、DNS 启动检查，以及文件描述符不足时恢复接收连接的修复。
- 现有订阅、规则、端口和节点组配置在升级时保留。

安装包使用 Developer ID 签名并经过 Apple 公证，适用于 Apple Silicon Mac，macOS 14 或更新版本。Xray 使用独立的配置目录和更新地址。

此前标为 `1.6.x` 的 Xray 包现已统一到 MyProxy 的版本规则。应用内的数字构建号继续递增，因此旧版仍可通过 Xray 通道更新。
