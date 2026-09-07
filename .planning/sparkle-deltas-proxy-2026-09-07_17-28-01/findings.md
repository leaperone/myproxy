# 调研与结论：sparkle-deltas-proxy

- 任务 ID：`sparkle-deltas-proxy-2026-09-07_17-28-01`
- 创建时间：`2026-09-07_17-28-01`

## 需求事实

- 用户要更新通道有增量更新，且更新下载可走代理（Mixed）
- 当前通道是 Nightly；Prod 已有 delta，Nightly 被脚本明确跳过

## 真实调用链

- 发布：`release.yml` → `scripts/release-macos.sh` → `generate_appcast` → 上传 zip / appcast / `dist/*.delta`；Nightly 指针 tag 只挂 appcast
- 运行时：`sparkle::init` / `set_channel` / `check` → `sparkle_bridge.m` `SPUStandardUpdaterController`
- Sparkle `SPUDownloader` 每次下载 `+[NSURLSessionConfiguration defaultSessionConfiguration]` 新建 session
- Mixed 生命周期在 `Supervisor::{adopt_running,start_with_catalog,disconnect_inner,is_running}`

## 调研结论

- `willDownloadUpdate:withRequest:` 改不了 session 代理；URLSession 不读 `HTTP_PROXY`
- 注入 `connectionProxyDictionary`（HTTP/HTTPS）即可让 appcast 与 zip/delta 走 Mixed CONNECT
- Nightly zip 在 `v*-nightly.*` prerelease，不在指针 tag `nightly`

## 技术决策

| 决策 | 证据 |
|---|---|
| 按通道筛选上一版 zip | Prod `--exclude-pre-releases`；Nightly 需反选并跳过 `nightly` |
| swizzle defaultSessionConfiguration | Sparkle 2 Downloader 每请求新建 session |
| fail-open | Mixed 未监听时保持系统默认，避免检查更新必失败 |

## 风险与边界

- `mixed_mode=direct` 时经 Mixed 的 GitHub 仍直连，可能被墙
- 启动自动检查可能早于 `connect_on_launch`；连接后 hook 再同步
- 主仓 dirty Mixed 调试不得进入本分支

## 参考指针

- `scripts/release-macos.sh` L78–L86
- `packaging/macos/sparkle_bridge.m`
- Sparkle `Downloader/SPUDownloader.m` `startDownloadWithRequest:`
