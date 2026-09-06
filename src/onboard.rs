use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogButtonProps, DialogFooter};
use gpui_kit::component::{v_flex, ActiveTheme, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

/// Official agent skill; keep this the only copy-paste source.
pub const AGENT_SKILL: &str = include_str!("../.agents/skills/agent/SKILL.md");

pub fn copy_agent_skill(cx: &App) {
    cx.write_to_clipboard(ClipboardItem::new_string(AGENT_SKILL.to_string()));
}

#[derive(Default, Deserialize, Serialize)]
struct OnboardState {
    #[serde(default)]
    cli_done: bool,
}

fn load() -> OnboardState {
    let Ok(path) = myproxy::paths::onboard_path() else {
        return OnboardState::default();
    };
    let Ok(data) = std::fs::read_to_string(path) else {
        return OnboardState::default();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

fn save(state: &OnboardState) {
    let Ok(path) = myproxy::paths::onboard_path() else {
        return;
    };
    if let Ok(data) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, data);
    }
}

pub fn mark_cli_done() {
    let mut state = load();
    if state.cli_done {
        return;
    }
    state.cli_done = true;
    save(&state);
}

pub fn should_prompt() -> bool {
    if !myproxy::cli_install::available() {
        return false;
    }
    if myproxy::cli_install::is_installed() {
        mark_cli_done();
        return false;
    }
    !load().cli_done
}

struct OnboardView {
    notice: String,
    installed: bool,
    copied: bool,
    on_result: Rc<dyn Fn(Result<PathBuf, String>, &mut App)>,
}

impl OnboardView {
    fn install(&mut self, cx: &mut Context<Self>) -> bool {
        if self.installed {
            return true;
        }
        match myproxy::cli_install::install() {
            Ok(path) => {
                mark_cli_done();
                self.installed = true;
                self.notice = format!("已安装到 {}。复制 skill 后关掉即可。", path.display());
                (self.on_result)(Ok(path), cx);
                cx.notify();
                false
            }
            Err(error) => {
                let message = format!("{error:#}");
                self.notice = format!("安装失败：{message}");
                (self.on_result)(Err(message), cx);
                cx.notify();
                false
            }
        }
    }
}

impl Render for OnboardView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let destination = myproxy::cli_install::destination()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "~/.cargo/bin/myproxyctl".into());
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .child("myproxyctl 让 Agent 或终端直接配置订阅、节点组和规则。点一下即可链到用户目录，并随应用更新。"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("安装路径：{destination}")),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("若终端找不到命令，把 ~/.cargo/bin 加进 PATH。"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("之后若打开「系统接管」，请到系统设置 › 通用 › 登录项与扩展 允许 myproxy。"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if self.copied {
                        "已复制官方 Agent skill，可贴给 Cursor / Codex。"
                    } else {
                        "复制官方 Agent skill，交给 Cursor / Codex 即可管理 myproxy。"
                    }),
            )
            .when(!self.notice.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(if self.installed {
                            theme.success
                        } else {
                            theme.danger
                        })
                        .child(self.notice.clone()),
                )
            })
    }
}

pub fn open(
    window: &mut Window,
    cx: &mut App,
    on_result: impl Fn(Result<PathBuf, String>, &mut App) + 'static,
) {
    if !should_prompt() {
        return;
    }
    let view = cx.new(|_| OnboardView {
        notice: String::new(),
        installed: false,
        copied: false,
        on_result: Rc::new(on_result),
    });
    window.open_dialog(cx, move |dialog, _, _| {
        dialog
            .title("安装命令行工具")
            .width(px(520.))
            .overlay_closable(true)
            .button_props(
                DialogButtonProps::default()
                    .on_ok({
                        let view = view.clone();
                        move |_, _, cx| view.update(cx, |this, cx| this.install(cx))
                    })
                    .on_cancel(|_, _, _| {
                        mark_cli_done();
                        true
                    }),
            )
            .on_close(|_, _, _| {
                mark_cli_done();
            })
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("onboard-later").label("稍后").on_click(
                            |_, window, cx| window.dispatch_action(Box::new(Cancel), cx),
                        ),
                    )
                    .child({
                        let view = view.clone();
                        Button::new("onboard-copy-skill")
                            .label("复制 Agent skill")
                            .on_click(move |_, _, cx| {
                                copy_agent_skill(cx);
                                view.update(cx, |this, cx| {
                                    this.copied = true;
                                    cx.notify();
                                });
                            })
                    })
                    .child(
                        Button::new("onboard-install")
                            .primary()
                            .label("安装 myproxyctl")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    Box::new(Confirm { secondary: false }),
                                    cx,
                                )
                            }),
                    ),
            )
            .child(view.clone())
    });
}
