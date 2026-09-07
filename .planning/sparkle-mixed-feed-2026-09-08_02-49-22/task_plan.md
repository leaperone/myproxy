# 任务计划：sparkle-mixed-feed

- 任务 ID：`sparkle-mixed-feed-2026-09-08_02-49-22`
- 创建时间：`2026-09-08_02-49-22`

## 目标

Mixed 开启时，应用内检查更新和下载不再走 CFNetwork 进程代理。改为 Rust 经 Mixed HTTP 按主机新建 CONNECT 拉取 GitHub release，Sparkle 只读本机 feed。

## 范围

- `src/updates.rs`：GitHub 主机白名单、分跳跟随、enclosure 改写
- `src/sparkle.rs`：本机 HTTP feed，流式转发 zip/delta
- `packaging/macos/sparkle_bridge.m`：去掉 NSURLSession 代理 swizzle
- `packaging/macos/Info.plist`：允许本机 HTTP
- `AGENTS.md`：更新 Mixed 更新路径说明

## 非目标

- 不改 Prod 版本号或正式 feed
- 不在 dirty main 上提交
- 不保留调试埋点
- 不修 NE 配置过大或系统接管 entitlement

## 关键约束

- 仓库规则：`AGENTS.md`
- Nightly 必须从已合并的 `origin/main` 发布
- 不记录订阅 URL；release URL 日志只写 host

## 修改路径

- `src/updates.rs`
- `src/sparkle.rs`
- `packaging/macos/sparkle_bridge.m`
- `packaging/macos/Info.plist`
- `AGENTS.md`

## 验证方式

- `cargo test --lib rewrite_enclosure allow_github_release_hosts resolve_absolute`
- 运行时已证实：分跳 Mixed HTTP 可拉 appcast；流式下载 33.8MB；无扩展名会导致 Sparkle 3000

## 验收标准

- Sparkle feed 指向 `http://127.0.0.1:<port>/appcast.xml`
- enclosure 路径保留 `.sparkle.zip` / `.delta`
- Mixed 在听时用 HTTP 代理拉 GitHub，每个 302 新开 CONNECT
- 无 CFNetwork HTTP/SOCKS 字典
- 无 debug session 文件写入

## 未确认事项

无。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [ ] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 不用 CFNetwork 代理 | HTTPS/SOCKS 跟 GitHub 302 到 release-assets 都会 TLS 失败 |
| 本机 feed + 文件名扩展 | Sparkle 按 URL 扩展名选解压器 |
| 流式转发 | 先整包缓冲再交给 Sparkle 时进度停滞 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| CFNetwork HTTPS 代理 -1200 | 1 | 改 Mixed HTTP 本机拉取 |
| SOCKS 同样 -1200 | 2 | 撤回 SOCKS |
| Sparkle 3000 无解压器 | 3 | enclosure 带原文件名 |
