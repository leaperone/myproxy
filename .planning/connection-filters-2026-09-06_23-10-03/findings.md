# 调研与结论：connection-filters

- 任务 ID：`connection-filters-2026-09-06_23-10-03`
- 创建时间：`2026-09-06_23-10-03`

## 需求事实

- 用户要连接页表格「每个列都能选择筛选」
- 额外要能选择是否展示直连连接
- 连接页现有列：进程、目标、协议、走向、上传、下载、时长、关闭

## 真实调用链

- `AppView::connections` 在已连接且 `traffic.connections` 非空时渲染表头 + 全量行
- 行数据来自 `controller::fetch` → `LiveConnection::from_raw`
- `chains` 反转后用 ` → ` 拼接；直连通常是单独的 `DIRECT`
- 空态只看 `connections.is_empty()`，与筛选无关
- 表头是静态文案，`connection_col` / `connection_header_row`

## 调研结论

- 现成下拉模式：`Button.dropdown_menu` + `PopupMenuItem.checked`（规则走向、外观菜单）
- 筛选应在 UI 渲染前过滤，不改 mihomo 轮询
- 有原始连接但筛选为空时必须仍渲染表头，否则无法改筛选
- 上传/下载/时长用现值筛选不合适

## 技术决策

| 决策 | 证据 |
|---|---|
| `ConnectionFilters` 放 controller | 与 `LiveConnection` 同模块，可 `cargo test --lib` |
| 直连 = hop `eq_ignore_ascii_case("DIRECT")` | `from_raw` 把 chains 拼成 `A → B`；直连为 `DIRECT` |
| 列选项来自「忽略本列后仍匹配」的行 | 换进程时不必先清其他列 |
| 默认 `show_direct = false` | 系统接管时直连会淹没有用行 |

## 风险与边界

- 目标列唯一值最多约 `UI_CONNECTION_CAP`（200），菜单需 scrollable
- 默认隐藏直连会改变当前「全显示」行为；开关可见
- 筛选不进 strategy，重启窗口重置

## 参考指针

- `src/ui.rs` `connections` / `connection_header_row` / `render_connection_row`
- `src/controller.rs` `LiveConnection::from_raw`
- `src/ui.rs` `via_menu` 作为下拉样板
