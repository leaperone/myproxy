# Safari 原生桥接协议

Safari WebExtension 不支持 `proxy` API，因此不能在 JavaScript 中直接安装 PAC。Safari 版本使用 `nativeMessaging` 把浏览器规则交给包含它的 macOS App Extension。

## 请求

扩展后台脚本发送：

```json
{
  "type": "browserProxyApply",
  "state": {
    "mode": "smart",
    "mixedHost": "127.0.0.1",
    "mixedPort": 7891,
    "rules": [
      { "pattern": "*.github.com", "action": "proxy" }
    ]
  }
}
```

原生 Safari 扩展实现 `NSExtensionRequestHandling.beginRequest(with:)`，校验消息后通过 myproxy 的 Host 私有 IPC 发送浏览器范围规则。规则的唯一来源仍是 myproxy 的策略快照；扩展配置只表示浏览器范围的意图。

## 原生侧约束

- 不把 `strategy.json` 或订阅 URL 暴露给 Safari JavaScript。
- 不让 Safari 直接访问 Mihomo controller。
- 通过应用组共享扩展状态，通过 Host IPC 执行变更。
- Safari 的匹配规则需要编译为 `Safari 应用 + 目标域名` 的求交规则，避免把浏览器规则误用于其他应用。
- Safari 原生 target 与现有 `macos/NetworkHost`、`macos/NetworkExtension` 一起签名，并由包含 App 打包。
