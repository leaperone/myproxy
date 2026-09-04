mod ui;

use gpui_kit::component::{Root, TitleBar};
use gpui_kit::*;

use ui::AppView;

fn main() {
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.run(move |cx| {
        gpui_kit::init(cx);
        cx.spawn(async move |cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(96.), px(72.)),
                        size: size(px(1120.), px(740.)),
                    })),
                    window_min_size: Some(size(px(880.), px(560.))),
                    ..TitleBar::window_options()
                },
                |window, cx| {
                    let view = cx.new(|cx| AppView::new(window, cx));
                    cx.new(|cx| Root::new(view, window, cx))
                },
            )
            .expect("failed to open window");
        })
        .detach();
    });
}
