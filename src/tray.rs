use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::*;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton as TrayMouseButton, MouseButtonState, TrayIcon, TrayIconBuilder,
    TrayIconEvent,
};

use myproxy::strategy::Strategy;
use myproxy::supervisor::Supervisor;

struct TrayKeepAlive {
    tray: TrayIcon,
    status: MenuItem,
    toggle: MenuItem,
}

impl Global for TrayKeepAlive {}

#[derive(Clone, PartialEq, Eq)]
struct MenuFace {
    connected: bool,
    status: String,
    action: String,
    tooltip: String,
}

pub fn install(cx: &mut App) {
    let initial = menu_face();
    match build_tray() {
        Ok(tray) => {
            tray.apply(&initial);
            cx.set_global(tray);
        }
        Err(err) => myproxy::log::warn("tray", format!("menu bar icon failed: {err:#}")),
    }
    let last = Arc::new(Mutex::new(initial));
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(1000))
            .await;
        let face = cx
            .background_executor()
            .spawn(async { menu_face() })
            .await;
        let displayed = last.lock().expect("tray menu face").clone();
        let face_changed = displayed != face;
        let mut clicks = Vec::new();
        let mut menus = Vec::new();
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            clicks.push(event);
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            menus.push(event);
        }
        if !face_changed && clicks.is_empty() && menus.is_empty() {
            continue;
        }
        cx.update(|cx| {
            for event in clicks {
                handle_tray_event(event, cx);
            }
            for event in menus {
                handle_menu_event(event, cx, displayed.connected);
            }
            if face_changed {
                if let Some(tray) = cx.try_global::<TrayKeepAlive>() {
                    tray.apply(&face);
                }
                *last.lock().expect("tray menu face") = face;
            }
        });
    })
    .detach();
}

fn build_tray() -> anyhow::Result<TrayKeepAlive> {
    let status = MenuItem::with_id("status", "未连接", false, None);
    let toggle = MenuItem::with_id("toggle", "连接", true, None);
    let open = MenuItem::with_id("open", "打开窗口", true, None);
    let apply = MenuItem::with_id("apply", "更新配置", true, None);
    let updates = MenuItem::with_id("updates", "检查更新", true, None);
    let quit = MenuItem::with_id("quit", "退出", true, None);
    let menu = Menu::new();
    menu.append(&status)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&toggle)?;
    menu.append(&open)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&apply)?;
    menu.append(&updates)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("myproxy")
        .with_icon(template_icon())
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    Ok(TrayKeepAlive {
        tray,
        status,
        toggle,
    })
}

impl TrayKeepAlive {
    fn apply(&self, face: &MenuFace) {
        self.status.set_text(&face.status);
        self.toggle.set_text(&face.action);
        if let Err(err) = self.tray.set_tooltip(Some(&face.tooltip)) {
            myproxy::log::debug("tray", format!("tooltip failed: {err:#}"));
        }
    }
}

fn menu_face() -> MenuFace {
    let strategy = match Strategy::load() {
        Ok(strategy) => strategy,
        Err(_) => {
            return MenuFace {
                connected: false,
                status: "未连接".into(),
                action: "连接".into(),
                tooltip: "myproxy · 未连接".into(),
            };
        }
    };
    let health = Supervisor::shared().observe(&strategy);
    if !health.wanted {
        return MenuFace {
            connected: false,
            status: "未连接".into(),
            action: "连接".into(),
            tooltip: "myproxy · 未连接".into(),
        };
    }
    if !health.ready {
        return MenuFace {
            connected: true,
            status: health.note.unwrap_or_else(|| "核心异常".into()),
            action: "断开".into(),
            tooltip: "myproxy · 核心异常".into(),
        };
    }
    let endpoint = format!("127.0.0.1:{}", strategy.mixed_port);
    let now = (!health.proxy_now.is_empty()).then_some(health.proxy_now.as_str());
    let mode = if strategy.system_extension {
        "系统接管"
    } else if strategy.tun {
        "TUN"
    } else {
        "Mixed"
    };
    let status = match now {
        Some(now) => format!("已连接 · {mode} · {now}"),
        None => format!("已连接 · {mode}"),
    };
    let tooltip = match now {
        Some(now) => format!("myproxy · 已连接 · {endpoint} · {now}"),
        None => format!("myproxy · 已连接 · {endpoint}"),
    };
    MenuFace {
        connected: true,
        status,
        action: "断开".into(),
        tooltip,
    }
}

fn handle_tray_event(event: TrayIconEvent, cx: &mut App) {
    let TrayIconEvent::Click {
        button: TrayMouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    else {
        return;
    };
    crate::show_main_window(cx);
}

fn handle_menu_event(event: MenuEvent, cx: &mut App, displayed_connected: bool) {
    match event.id.as_ref() {
        "open" => crate::show_main_window(cx),
        "toggle" if displayed_connected => disconnect_async(cx),
        "toggle" => connect(cx),
        "apply" => apply(cx),
        "updates" => crate::sparkle::check(),
        "quit" => disconnect_and_quit(cx),
        _ => {}
    }
}

fn connect(cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            match Strategy::load() {
                Ok(strategy) => {
                    if let Err(err) = Supervisor::shared().connect(&strategy) {
                        myproxy::log::error("tray", format!("connect failed: {err:#}"));
                    }
                }
                Err(err) => myproxy::log::error("tray", format!("load strategy failed: {err:#}")),
            }
        })
        .detach();
}

fn disconnect() {
    if let Err(err) = Supervisor::shared().disconnect() {
        myproxy::log::error("tray", format!("disconnect failed: {err:#}"));
    }
}

fn disconnect_async(cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            disconnect();
        })
        .detach();
}

fn disconnect_and_quit(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor()
            .spawn(async move {
                if let Err(err) = Supervisor::shared().shutdown() {
                    myproxy::log::error("tray", format!("shutdown failed: {err:#}"));
                }
            })
            .await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

fn apply(cx: &mut App) {
    cx.background_executor()
        .spawn(async move {
            match Strategy::load().and_then(|strategy| Supervisor::shared().apply(&strategy)) {
                Ok(_) => {}
                Err(err) => myproxy::log::error("tray", format!("apply failed: {err:#}")),
            }
        })
        .detach();
}

fn template_icon() -> Icon {
    const SIZE: u32 = 32;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let r = (dx * dx + dy * dy).sqrt();
            let ring = r > 9.0 && r < 13.5;
            let gap = dy < -2.0 && dx.abs() < 4.5 && r < 13.5;
            if ring && !gap {
                let i = ((y * SIZE + x) * 4) as usize;
                rgba[i] = 0;
                rgba[i + 1] = 0;
                rgba[i + 2] = 0;
                rgba[i + 3] = 255;
            }
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).expect("tray icon")
}
