# 发现

## 调用链

`sparkle::check` → `feedURLStringForUpdater` → Sparkle Downloader → GitHub appcast 302 → `release-assets.githubusercontent.com`。

旧实现给 `NSURLSessionConfiguration` 套 Mixed HTTP/HTTPS 或 SOCKS。CFNetwork 复用 CONNECT 跟 302 时 TLS 失败（`NSURLError -1200` / `-9816`）。

## 证据

- Mixed HTTP 对 github.com 拿 302 成功；对 CDN URL 单独 CONNECT 成功（200 / 3464）
- `curl -L` 与 CFNetwork 跟跳失败
- SOCKS 调试包仍失败，失败 URL 仍是 release-assets appcast
- 本机 feed 拉取成功后，`/asset` 无扩展名导致 `SUUnarchivingError 3000`
- 流式下载 33818500 字节 / 64931ms 成功

## 结论

Sparkle 只应访问本机 HTTP。Rust 用 ureq + Mixed HTTP、`redirects(0)` 逐跳请求。enclosure 路径必须带 `.zip` / `.delta`。
