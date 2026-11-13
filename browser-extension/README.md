# MyProxy Browser Extension

浏览器侧的按网站代理入口。节点、订阅、DNS 和分流策略仍由 myproxy 管理；扩展只保存浏览器范围的匹配规则，并把匹配请求送入 myproxy 的 Mixed 端口。

## 支持的模式

- `自动切换`：只有 `代理` 规则匹配的网站进入 myproxy，其他请求直连。
- `始终代理`：默认进入 myproxy，`直连` 规则作为例外。
- `直连`：关闭浏览器代理。
- `系统代理`：使用浏览器当前的系统代理配置。

规则支持域名、子域名和 `*` / `?` 通配符，例如 `*.github.com`。

## 构建

```sh
scripts/package-browser-extension.sh chromium
scripts/package-browser-extension.sh firefox
scripts/package-browser-extension.sh safari
```

输出在 `target/browser-extension/<browser>/`。Chrome/Edge 可以在扩展管理页加载 Chromium 输出；Firefox 可以临时加载 Firefox 输出。

Safari 使用同一份资源目录，但 Safari 没有 `proxy` WebExtension API。Safari 输出需要由 Xcode 的 Safari Web Extension App target 打包，并由包含 App 的原生扩展通过 `nativeMessaging` 把规则交给 myproxy Host。当前 Safari 原生桥接协议见 `safari/native-bridge.md`。

## 运行约定

默认入口是 `127.0.0.1:7891`，即 myproxy 的 Mixed HTTP/SOCKS 入口。扩展不会保存订阅 URL、节点密码或节点列表。
