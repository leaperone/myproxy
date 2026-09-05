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
    _tray: TrayIcon,
}

impl Global for TrayKeepAlive {}

pub fn install(cx: &mut App) {
    match build_tray() {
        Ok(tray) => cx.set_global(TrayKeepAlive { _tray: tray }),
        Err(err) => myproxy::log::warn("tray", format!("menu bar icon failed: {err:#}")),
    }
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        let mut clicks = Vec::new();
        let mut menus = Vec::new();
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            clicks.push(event);
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            menus.push(event);
        }
        if clicks.is_empty() && menus.is_empty() {
            continue;
        }
        cx.update(|cx| {
            for event in clicks {
                handle_tray_event(event, cx);
            }
            for event in menus {
                handle_menu_event(event, cx);
            }
        });
    })
    .detach();
}

fn build_tray() -> anyhow::Result<TrayIcon> {
    let open = MenuItem::with_id("open", "打开窗口", true, None);
    let connect = MenuItem::with_id("connect", "连接", true, None);
    let disconnect = MenuItem::with_id("disconnect", "断开", true, None);
    let apply = MenuItem::with_id("apply", "更新配置", true, None);
    let updates = MenuItem::with_id("updates", "检查更新", true, None);
    let quit = MenuItem::with_id("quit", "退出", true, None);
    let menu = Menu::new();
    menu.append(&open)?;
    menu.append(&connect)?;
    menu.append(&disconnect)?;
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
    Ok(tray)
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

fn handle_menu_event(event: MenuEvent, cx: &mut App) {
    match event.id.as_ref() {
        "open" => crate::show_main_window(cx),
        "connect" => connect(cx),
        "disconnect" => disconnect_async(cx),
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
