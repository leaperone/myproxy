# 任务计划：sparkle-deltas-proxy

- 任务 ID：`sparkle-deltas-proxy-2026-09-07_17-28-01`
- 创建时间：`2026-09-07_17-28-01`

## 目标

Prod 与 Nightly 更新通道都生成 Sparkle 增量包；应用内检查与下载在核心已连接时走 Mixed HTTP 代理。

## 范围

- Nightly `generate_appcast` 拉取近期 Nightly zip（跳过 `nightly` 指针），产出并上传 `*.delta`
- Sparkle `NSURLSession` 在 Mixed 就绪时使用 `127.0.0.1:mixed_port`
- 设置页提示增量与走代理；README / AGENTS 同步

## 非目标

- 不改分流规则、NE、TUN
- 不新增专用更新 inbound
- 不强制系统代理，核心未监听时回退系统/直连
- 不混入主仓未提交的 Mixed 调试改动

## 关键约束

- Nightly 只对 Nightly zip 做 delta，不与 Prod 交叉
- Sparkle 无公开 session 配置 API，对 `defaultSessionConfiguration` 注入 `connectionProxyDictionary`
- 核心断开后清除代理，避免指向已关闭的 Mixed

## 修改路径

- `scripts/release-macos.sh`、`scripts/sparkle_previous_tags.py`
- `packaging/macos/sparkle_bridge.m`、`src/sparkle.rs`、`src/supervisor.rs`、`src/main.rs`
- `src/ui.rs`、`README.md`、`AGENTS.md`

## 验证方式

- `sparkle_previous_tags.py` 单测
- `cargo test`（supervisor / 现有套件）
- `cargo check --bins --features sparkle`（macOS）
- 对照现有 Nightly 列表确认会选中可下载的旧 zip

## 验收标准

- Nightly 发布目录在存在上一版 zip 时产出 delta，且 enclosure 指向该次 tag
- 核心监听后 Sparkle session 走 Mixed；断开后不再强制 Mixed
- 检查更新前同步当前 Mixed 端口

## 未确认事项

无

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| Nightly 也拉最近 6 个同通道 zip | 与 Prod 相同，Sparkle `--maximum-deltas 5` |
| Mixed HTTP/HTTPS CONNECT，不另开 inbound | Mixed 已是 HTTP+SOCKS；避免新端口 |
| Supervisor 实例 hook 通知二进制 Sparkle | Sparkle FFI 只链在 app，避免全局静态 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| worktree 被切到 `feat/ne-gfw-via` | 1 | 改动文件在两提交间无 diff，切回 `feat/sparkle-deltas-proxy` |
| `gh` 直连 api.github.com 失败 | 1 | 经 Mixed `127.0.0.1:7891` 拉 release 列表 |
| toolchain rustfmt 未装 | 1 | 未跑 `cargo fmt --check`；改动为手工格式 |
