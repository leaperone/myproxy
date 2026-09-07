# GFWList 分流支持

## 目标
为 myproxy 增加 Clash 风格 GFWList 导入与分流能力，避免未命中规则全部直连。

## 范围
- 支持以 URL 或本地文件导入常见 GFWList 文本（Base64/Adblock 域名条目）。
- 持久化来源、代理组、缓存和刷新状态。
- 编译为 mihomo 规则，保留本地直连规则优先。
- 提供 CLI 能力并让 apply 使用缓存；必要的 UI 入口按现有规则/订阅页面最小接入。

## 非目标
- 不改变现有显式规则优先级和 Telegram/局域网行为。
- 不把所有普通网站默认改成代理；GFWList 语义是列表内代理、列表外按 MATCH。
- 不输出订阅 URL、密钥或完整规则内容。

## 约束
- 遵守 AGENTS.md、最小改动、兼容已有 strategy.json。
- 默认代理组使用现有 default/PROXY 解析逻辑；来源不可用时使用缓存。

## 修改路径
- src/strategy.rs：来源与缓存元数据。
- src/rule_provider.rs 或 catalog：抓取、解码、解析和缓存。
- src/compile.rs：规则集生成。
- src/bin/myproxyctl.rs、src/ui.rs：导入/刷新/状态入口。

## 验证方式
- 单元测试覆盖 Base64、Adblock、域名归一化、缓存降级和编译顺序。
- cargo fmt/check/test/build，git diff --check。
- CLI 只读/导入/apply 回归。

## 验收标准
- 可以配置一个 GFWList URL 或文件并刷新。
- 生成的 mihomo rules 在用户规则前、MATCH 前，列表域名走目标组。
- GFWList 请求失败不丢缓存，状态可见。
