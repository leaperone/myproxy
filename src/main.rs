mod appearance;
mod sparkle;
mod ui;
#[cfg(target_os = "macos")]
mod tray;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use ui::AppView;

fn main() {
    myproxy::log::init();
    myproxy::log::info("main", "myproxy start");
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.run(move |cx| {
        gpui_kit::init(cx);
        appearance::apply_saved(None, cx);
        #[cfg(target_os = "macos")]
        tray::install(cx);
        sparkle::init();
        cx.spawn(async move |cx| {
            cx.update(|cx| {
                open_main_window(cx);
            });
        })
        .detach();
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
