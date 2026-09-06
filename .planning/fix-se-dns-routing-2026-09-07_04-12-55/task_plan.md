# 修复 SE DNS 路由与重复实例

## 目标
定位并修复系统接管模式下 DNS 异常，以及确认 myproxy 是否可能启动多个宿主实例。

## 范围
- 核对 DNSProxyProvider 的公共/局域网 DNS 路由和失败处理。
- 核对 host 单实例与启动项行为，必要时补最小保护。
- 在 Mac mini 上验证扩展、DNS 流量和实例数量。

## 非目标
- 不修改用户订阅、节点组或规则语义。
- 不保留 MClash 扩展并行运行。

## 约束
- 遵循仓库 AGENTS.md；不暴露订阅 URL 或密钥。
- 只做与现象直接相关的改动。

## 修改路径
- `macos/NetworkExtension/` DNS/透明代理链路。
- host 启动与单实例相关代码（若证据确认需要）。

## 验证方式
- Swift/Rust 针对性测试与构建。
- Mac mini：`myproxyctl status`、系统扩展状态、监听端口、统一日志、DNS 查询、进程/窗口实例检查。

## 验收标准
- SE DNSProxy 保持 connected，公共 DNS 经过预期 SOCKS 路径。
- 局域网/`.local` DNS 不再产生持续 `local:53 can't resolve ip` 错误，或有明确可验证的降级行为。
- myproxy 不会启动两个宿主实例；重复启动会直接退出。
