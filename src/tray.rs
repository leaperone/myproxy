use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::*;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton as TrayMouseButton, MouseButtonState, TrayIcon, TrayIconBuilder,
    TrayIconEvent,
};

use myproxy::network_extension;
use myproxy::strategy::Strategy;
use myproxy::supervisor::{OperationState, Supervisor};

static DISPATCHING: AtomicBool = AtomicBool::new(false);

struct TrayKeepAlive {
    tray: TrayIcon,
    status: MenuItem,
    toggle: MenuItem,
    apply: MenuItem,
}

impl Global for TrayKeepAlive {}

#[derive(Clone, PartialEq, Eq)]
struct MenuFace {
    busy: bool,
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
    let observed = Arc::new(Mutex::new(initial.clone()));
    let health_face = observed.clone();
    // Health work may recover the core; event consumption runs independently.
    cx.spawn(async move |cx| loop {
        let health_face = health_face.clone();
        cx.background_executor()
            .spawn(async move {
                if !Supervisor::shared().is_busy() {
                    if let Ok(strategy) = Strategy::load() {
                        Supervisor::shared().observe(&strategy);
                    }
                }
                *health_face.lock().expect("tray observed face") = menu_face();
            })
            .await;
        cx.background_executor()
            .timer(Duration::from_millis(1500))
            .await;
    })
    .detach();
    let last = Arc::new(Mutex::new(initial));
    cx.spawn(async move |cx| loop {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        let mut face = observed.lock().expect("tray observed face").clone();
        if DISPATCHING.load(Ordering::Acquire) || Supervisor::shared().is_busy() {
            if !face.busy {
                face.status = "正在处理操作…".into();
            }
            face.busy = true;
            face.action = "处理中…".into();
        }
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
                handle_menu_event(event, cx);
            }
            if DISPATCHING.load(Ordering::Acquire) || Supervisor::shared().is_busy() {
                if !face.busy {
                    face.status = "正在处理操作…".into();
                }
                face.busy = true;
                face.action = "处理中…".into();
            }
            if face_changed || displayed != face {
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
    let about = MenuItem::with_id("about", "关于 MyProxy", true, None);
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
    menu.append(&about)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit)?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("MyProxy")
        .with_icon(template_icon())
        .with_icon_as_template(true)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    Ok(TrayKeepAlive {
        tray,
        status,
        toggle,
        apply,
    })
}

impl TrayKeepAlive {
    fn apply(&self, face: &MenuFace) {
        self.status.set_text(&face.status);
        self.toggle.set_text(&face.action);
        self.toggle.set_enabled(!face.busy);
        self.apply.set_enabled(!face.busy);
        if let Err(err) = self.tray.set_tooltip(Some(&face.tooltip)) {
            myproxy::log::debug("tray", format!("tooltip failed: {err:#}"));
        }
    }
}

fn menu_face() -> MenuFace {
    let supervisor = Supervisor::shared();
    let operation = supervisor.operation_state();
    let busy = operation.is_busy() || DISPATCHING.load(Ordering::Acquire);
    let health = supervisor.last_health();
    let runtime = supervisor.runtime_identity();
    let extension = network_extension::status();
    let mut status = match operation {
        OperationState::Connecting => "正在连接…".into(),
        OperationState::Applying => "正在应用策略…".into(),
        OperationState::Disconnecting => "正在断开…".into(),
        _ if busy => "正在处理操作…".into(),
        _ if health.ready && runtime.is_some() => format!(
            "Mixed 已就绪 · {} · 系统接管 {} · DNS {}",
            runtime
                .map(|identity| format!("127.0.0.1:{}", identity.mixed_port))
                .unwrap_or_default(),
            extension.phase_label(),
            extension.dns_label()
        ),
        _ if supervisor.wanted() => health.note.clone().unwrap_or_else(|| "核心尚未就绪".into()),
        _ => "未连接".into(),
    };
    if !busy && operation == OperationState::Error {
        status = health
            .note
            .unwrap_or_else(|| "上次操作失败，请在窗口查看详情".into());
    }
    MenuFace {
        busy,
        action: if busy {
            "处理中…"
        } else if supervisor.wanted() {
            "断开"
        } else {
            "连接"
        }
        .into(),
        tooltip: format!("MyProxy · {status}"),
        status,
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

fn handle_menu_event(event: MenuEvent, cx: &mut App) {
    match event.id.as_ref() {
        "open" => crate::show_main_window(cx),
        "about" => crate::show_about(),
        "toggle" | "apply" => {
            if Supervisor::shared().is_busy() || DISPATCHING.swap(true, Ordering::AcqRel) {
                return;
            }
            let apply = event.id.as_ref() == "apply";
            let disconnect = !apply && Supervisor::shared().wanted();
            if let Some(tray) = cx.try_global::<TrayKeepAlive>() {
                tray.status.set_text(if apply {
                    "正在应用策略…"
                } else if disconnect {
                    "正在断开…"
                } else {
                    "正在连接…"
                });
                tray.toggle.set_text("处理中…");
                tray.toggle.set_enabled(false);
                tray.apply.set_enabled(false);
            }
            cx.background_executor()
                .spawn(async move {
                    let supervisor = Supervisor::shared();
                    let result = if disconnect {
                        supervisor.disconnect()
                    } else {
                        Strategy::load().and_then(|strategy| {
                            if apply {
                                let cached = myproxy::catalog::Catalog::load()
                                    .ok()
                                    .is_some_and(|catalog| catalog.matches_strategy(&strategy));
                                if cached {
                                    supervisor.apply_cached(&strategy).map(|_| ())
                                } else {
                                    supervisor.apply(&strategy).map(|_| ())
                                }
                            } else {
                                supervisor.connect(&strategy)
                            }
                        })
                    };
                    if let Err(err) = result {
                        myproxy::log::error("tray", format!("operation failed: {err:#}"));
                    }
                    DISPATCHING.store(false, Ordering::Release);
                })
                .detach();
        }
        "updates" => crate::sparkle::check(),
        "quit" => crate::quit_app(cx),
        _ => {}
    }
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
