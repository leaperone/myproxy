# 调研结论

- 侧边栏在 `src/ui.rs` 的 `AppView::sidebar`，宽度目前为 216px，使用 `SidebarHeader`、`SidebarGroup`、`SidebarMenuItem`。
- 导航项由 `nav_item` 统一创建，点击调用 `select_page`，不可改变行为。
- 顶部标题栏已有连接状态；侧边栏底部当前只显示英文节点统计。
- 主题颜色通过 `Theme` 提供，图标使用 `IconName`。
- GPUI Kit 的 SidebarMenuItem 已内置 hover/active 样式，本次保留现有点击行为；不修改组件库。该组件当前以 div 点击处理，键盘可访问性未在本任务范围内改动。
- Sidebar 外层默认已有内容内边距，Header 采用 p_2 以保持品牌内容与导航共享 leading edge。
- 连接状态可由现有 `connected`、`wanted`、`busy`、`traffic_error`、`proxy_error` 派生，无需新增状态。
