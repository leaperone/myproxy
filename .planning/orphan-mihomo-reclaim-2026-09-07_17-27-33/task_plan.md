# 任务计划：orphan-mihomo-reclaim

- 任务 ID：`orphan-mihomo-reclaim-2026-09-07_17-27-33`
- 创建时间：`2026-09-07_17-27-33`

## 目标

`disconnect` / `connect` 必须清掉本机残留的 bundled mihomo。不能只信 pid 文件，也不能在「pid 没有 listen」时放过真正占着 Mixed / DNS 的孤儿进程。

同时把 Mini 的 Mixed 从 `global` 改成 `proxy`，并重启一次清掉已终止、待卸载的旧 System Extension。

## 范围

- `src/supervisor.rs`：disconnect 时按 exe 识别并终止本实例的 leftover mihomo；回收 Mixed / controller / NE SOCKS / DNS 端口。
- `src/compile.rs`：抽出 DNS listen 端口常量，避免 1053 漂移。
- `AGENTS.md`：补一句 disconnect 会回收孤儿核心。
- Mini：`mixed-mode proxy` + apply；重启主机卸旧 SE。

## 非目标

- 不杀第三方 `mihomo` / Clash 二进制。
- 不改 Mixed / 系统接管的 routing mode 语义。
- 不在本任务发 Nightly；修完走 PR + preflight。

## 关键约束

- 主 checkout 有未提交改动，只在 `.worktrees/feat-orphan-mihomo-reclaim` 写代码。
- 识别条件：进程名为 `mihomo`，且 exe 是当前 `bundled_mihomo()` 或位于 `myproxy.app` 内。
- 不打印订阅 URL。

## 修改路径

- `src/supervisor.rs`：`disconnect_inner` → `reclaim_owned_mihomo`
- `src/compile.rs`：`DNS_LISTEN_PORT`
- `AGENTS.md`

## 验证方式

- 单测：`is_our_mihomo_exe` 路径判定；`owned_listen_ports` 端口集合。
- `cargo test --lib -- supervisor:: compile::`
- Mini：mixed_mode=proxy；重启后单核心、SE、7891/1053。

## 验收标准

- disconnect 会 SIGTERM/SIGKILL pid 文件、本 bundle 的 `mihomo`、以及占着本实例端口的本 bundle 进程。
- 「pid 没有 listen」不再被忽略并留下占口孤儿。
- Mini Mixed 为 `proxy`；重启后旧 SE 不再处于 terminated-waiting-uninstall。

## 未确认事项

无。

## 执行状态

- [x] 完成只读探索并确认真实调用链
- [x] 完成实现
- [x] 完成验证
- [x] 完成交付前收敛检查

## 决策

| 决策 | 理由 |
|---|---|
| 按 exe 识别，不按端口无差别杀进程 | 避免故意误杀用户自己的 clash/mihomo |
| 额外 `pgrep -x mihomo` 再过滤 exe | Mini 孤儿不在 pid 文件、cmdline 也和后来启动的不一致 |
| SIGTERM 后仍活则 SIGKILL | 杜绝占口 |

## 错误与处理

| 错误 | 尝试 | 处理结果 |
|---|---:|---|
| 新建 worktree 后被切到 `feat/app-menu-about-updates` | 1 | 改回 `feat/orphan-mihomo-reclaim` @ origin/main |
