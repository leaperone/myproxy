# 任务计划：se-dns-orphan-reclaim

- 任务 ID：`se-dns-orphan-reclaim-2026-09-10_15-33-21`
- 创建时间：`2026-09-10_15-33-21`

## 目标

连接/应用时必须回收本应用残留 mihomo，新核必须自己占有 Mixed 和 DNS 端口。系统接管启用 DNS 时，停不掉旧 NEDNSProxy 不能挡住重新保存配置。

## 范围

- `src/supervisor.rs`：本进程未托管的核心一律重连；spawn 后就绪以子进程占用端口为准；lsof/pgrep 用绝对路径；等待 DNS 端口释放
- `macos/NetworkHost/HostBridge.swift`：DNS disable 权限失败时继续 configureAndEnable

## 非目标

- 不发 Nightly（除非用户再要）
- 不改规则页 / GFWList 语义
- 不在此任务卸载需重启的旧 System Extension

## 关键约束

- 连接页与 CLI 仍可通过 pid 文件判断核心是否在跑
- 本进程 child 为空时不得把别人的 7891 当成启动成功
- 不打印订阅 URL

## 修改路径

- `src/supervisor.rs`
- `macos/NetworkHost/HostBridge.swift`

## 验证方式

- `cargo test --lib supervisor::`

## 验收标准

- 本进程未 spawn 的核心会走 disconnect + start
- 就绪检查要求 child 占用 Mixed（SE/TUN 时还要占用 1053）
- 有 supervisor 测试覆盖 reconnect 与端口归属

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
| child 为空则强制重连 | Sparkle/CLI 新进程会把 pid 文件当成已运行并 reload，留下占 1053 的孤儿 |
| 就绪看 child 的端口 | 1579 占着 7891/1053 时，新核会被误判为已就绪 |
| DNS disable 权限失败继续 enable | NEDNSProxyErrorDomain 1 会挡住重新写入系统 DNS |

## 错误与处理

无。
