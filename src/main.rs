mod appearance;
mod onboard;
mod sparkle;
mod ui;
#[cfg(target_os = "macos")]
mod tray;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use myproxy::login_item;
use myproxy::strategy::Strategy;
use myproxy::supervisor::Supervisor;

use ui::AppView;

fn main() {
    myproxy::log::init();
    let _instance_guard = match myproxy::instance::InstanceGuard::acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => {
            myproxy::log::info("main", "another myproxy instance is already running");
            return;
        }
        Err(err) => {
            myproxy::log::error("main", format!("acquire instance lock failed: {err}"));
            return;
        }
    };
    myproxy::log::info("main", "myproxy start");
    let strategy = Strategy::load().unwrap_or_else(|err| {
        myproxy::log::error("main", format!("load strategy failed: {err:#}"));
        Strategy::default()
    });
    myproxy::log::set_developer(strategy.developer_mode);
    myproxy::log::info(
        "main",
        format!(
            "launch login={} silent={} lite={} connect={}",
            strategy.launch_at_login,
            strategy.silent_launch,
            strategy.lite_mode,
            strategy.connect_on_launch
        ),
    );
    if let Err(err) = login_item::sync(strategy.launch_at_login) {
        myproxy::log::warn("login", format!("{err:#}"));
    }
    let lite = strategy.lite_mode;
    Supervisor::shared().adopt_running(
        strategy.tun,
        strategy.system_extension,
        strategy.mixed_port,
    );
    Supervisor::shared().sync_wanted_on_launch();
    let show_window = !lite && !strategy.silent_launch;
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.on_reopen(show_main_window);
    app.run(move |cx| {
        gpui_kit::init(cx);
        appearance::apply_saved(None, cx);
        #[cfg(target_os = "macos")]
        {
            if lite {
                set_accessory(true);
            }
            tray::install(cx);
        }
        sparkle::set_channel(strategy.update_channel.unwrap_or_default());
        sparkle::init();
        if strategy.connect_on_launch {
            let strategy = strategy.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(err) = Supervisor::shared().connect(&strategy) {
                        myproxy::log::error("main", format!("connect on launch failed: {err:#}"));
                    }
                })
                .detach();
        }
        if show_window {
            cx.spawn(async move |cx| {
                cx.update(|cx| {
                    open_main_window(cx);
                });
            })
            .detach();
        }
    });
}

fn window_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(96.), px(72.)),
            size: size(px(1120.), px(740.)),
        })),
        window_min_size: Some(size(px(880.), px(560.))),
        ..TitleBar::window_options()
    }
}

fn open_main_window(cx: &mut App) {
    cx.open_window(window_options(), |window, cx| {
        let view = cx.new(|cx| AppView::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    })
    .expect("failed to open window");
}

pub(crate) fn show_main_window(cx: &mut App) {
    #[cfg(target_os = "macos")]
    set_accessory(false);
    let handles = cx.windows();
    if handles.is_empty() {
        open_main_window(cx);
        return;
    }
    for handle in handles {
        let _ = handle.update(cx, |_, window, _| {
            window.activate_window();
        });
    }
}

#[cfg(target_os = "macos")]
fn set_accessory(accessory: bool) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};

    unsafe {
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        if app.is_null() {
            return;
        }
        let policy: i64 = if accessory { 1 } else { 0 };
        let _: objc::runtime::BOOL = msg_send![app, setActivationPolicy: policy];
    }
}
