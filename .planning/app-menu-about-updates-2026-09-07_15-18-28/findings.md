# 调研结论

- `src/main.rs` 从不调用 `cx.set_menus`。GPUI 不会自动生成应用菜单，所以菜单栏左侧 **MyProxy** 点开没有 About / Check for Updates。
- 右侧状态栏 extra（`src/tray.rs`）已经有「关于 MyProxy」「检查更新」，左键 `with_menu_on_left_click(true)`。托盘「关于」现在只是打开主窗口。
- Sparkle 只在 `sparkle` feature + `.app` 包里初始化；`sparkle::check()` 已存在。`SPUStandardUpdaterController` 不会自己往空的应用菜单里插项。
- GPUI `App::on_action` 写入 `global_action_listeners` 后，无窗口时 `is_action_available` 仍为 true，菜单项可点。
- 轻量模式会 `set_accessory(true)`，此时左侧应用菜单不出现，只剩右侧 extra。
