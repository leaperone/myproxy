use std::fs;
use std::path::PathBuf;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::{App, Window, WindowAppearance};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
    System,
}

impl Appearance {
    pub fn load() -> Self {
        preference_path()
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|raw| parse(&raw))
            .unwrap_or(Self::System)
    }

    pub fn save(self) {
        let Some(path) = preference_path() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, self.as_str());
    }

    pub fn apply(self, window: Option<&mut Window>, cx: &mut App) {
        match self {
            Self::Light => {
                cx.set_window_appearance(Some(WindowAppearance::Light));
                Theme::change(ThemeMode::Light, window, cx);
            }
            Self::Dark => {
                cx.set_window_appearance(Some(WindowAppearance::Dark));
                Theme::change(ThemeMode::Dark, window, cx);
            }
            Self::System => {
                cx.set_window_appearance(None);
                Theme::sync_system_appearance(window, cx);
            }
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }
}

pub fn apply_saved(window: Option<&mut Window>, cx: &mut App) {
    Appearance::load().apply(window, cx);
}

fn parse(raw: &str) -> Option<Appearance> {
    match raw.trim() {
        "light" => Some(Appearance::Light),
        "dark" => Some(Appearance::Dark),
        "system" => Some(Appearance::System),
        _ => None,
    }
}

fn preference_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/myproxy/appearance"))
}
