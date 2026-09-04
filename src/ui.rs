use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt, Theme, TitleBar,
};
use gpui_kit::*;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Subscriptions,
    Groups,
    Routing,
    Connections,
    Settings,
}

pub struct AppView {
    page: Page,
    connected: bool,
    down_bps: f64,
    up_bps: f64,
}

impl AppView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(900))
                .await;
            if this
                .update(cx, |this, cx| {
                    this.tick_traffic();
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        Self {
            page: Page::Overview,
            connected: true,
            down_bps: 2_400_000.0,
            up_bps: 180_000.0,
        }
    }

    fn tick_traffic(&mut self) {
        if !self.connected {
            self.down_bps *= 0.35;
            self.up_bps *= 0.35;
            return;
        }
        let jitter = |base: f64| {
            let n = rand::random::<f64>() * 0.28 - 0.12;
            (base * (1.0 + n)).max(12_000.0)
        };
        self.down_bps = jitter(2_350_000.0);
        self.up_bps = jitter(175_000.0);
    }

    fn select_page(
        &self,
        cx: &mut Context<Self>,
        page: Page,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity();
        move |_, _, app| {
            entity.update(app, |this, cx| {
                this.page = page;
                cx.notify();
            });
        }
    }

    fn toggle_connected(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity();
        move |_, _, app| {
            entity.update(app, |this, cx| {
                this.connected = !this.connected;
                if !this.connected {
                    this.down_bps = 0.0;
                    this.up_bps = 0.0;
                }
                cx.notify();
            });
        }
    }

    fn nav_item(
        &self,
        cx: &mut Context<Self>,
        page: Page,
        label: &'static str,
        icon: IconName,
    ) -> SidebarMenuItem {
        SidebarMenuItem::new(label)
            .icon(icon)
            .active(self.page == page)
            .on_click(self.select_page(cx, page))
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(self.title_bar(cx, &theme))
            .child(
                h_flex()
                    .id("shell")
                    .flex_1()
                    .overflow_hidden()
                    .child(self.sidebar(cx, &theme))
                    .child(self.page_view(cx, &theme)),
            )
    }
}

impl AppView {
    fn title_bar(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let connected = self.connected;
        let mut toggle = Button::new("toggle-core").small();
        toggle = if connected {
            toggle.danger().label("Disconnect")
        } else {
            toggle.primary().label("Connect")
        };

        TitleBar::new().child(
            h_flex()
                .id("title-contents")
                .w_full()
                .items_center()
                .justify_between()
                .pr_3()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child("myproxy"))
                        .child(pill(theme, "DEMO", theme.warning)),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(status_dot(theme, connected))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(if connected {
                                    "Connected · mixed 7890"
                                } else {
                                    "Disconnected"
                                }),
                        )
                        .child(toggle.on_click(self.toggle_connected(cx))),
                ),
        )
    }

    fn sidebar(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        Sidebar::new("nav")
            .w(px(216.))
            .header(
                SidebarHeader::new().child(
                    v_flex()
                        .gap(px(2.))
                        .child(div().text_sm().font_semibold().child("Control"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("mihomo · mock session"),
                        ),
                ),
            )
            .child(
                SidebarGroup::new("Session").child(
                    SidebarMenu::new()
                        .child(self.nav_item(
                            cx,
                            Page::Overview,
                            "Overview",
                            IconName::LayoutDashboard,
                        ))
                        .child(self.nav_item(
                            cx,
                            Page::Subscriptions,
                            "Subscriptions",
                            IconName::Inbox,
                        ))
                        .child(self.nav_item(cx, Page::Groups, "Node Groups", IconName::Folder))
                        .child(self.nav_item(cx, Page::Routing, "App Routing", IconName::Map)),
                ),
            )
            .child(
                SidebarGroup::new("Monitor").child(
                    SidebarMenu::new()
                        .child(self.nav_item(
                            cx,
                            Page::Connections,
                            "Connections",
                            IconName::Network,
                        ))
                        .child(self.nav_item(cx, Page::Settings, "Settings", IconName::Settings)),
                ),
            )
            .footer(
                SidebarFooter::new().child(
                    v_flex()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Exclude filter"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .child("流量|剩余|到期|官网"),
                        ),
                ),
            )
    }

    fn page_view(&self, _cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        v_flex()
            .id("page")
            .flex_1()
            .h_full()
            .overflow_hidden()
            .p_6()
            .gap_5()
            .bg(theme.background)
            .child(match self.page {
                Page::Overview => self.overview(theme).into_any_element(),
                Page::Subscriptions => self.subscriptions(theme).into_any_element(),
                Page::Groups => self.groups(theme).into_any_element(),
                Page::Routing => self.routing(theme).into_any_element(),
                Page::Connections => self.connections(theme).into_any_element(),
                Page::Settings => self.settings(theme).into_any_element(),
            })
    }

    fn overview(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_5()
            .child(page_title(
                theme,
                "Overview",
                "Mock session. Connect toggles numbers only.",
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(metric_card(
                        theme,
                        "Status",
                        if self.connected {
                            "Connected"
                        } else {
                            "Idle"
                        },
                        if self.connected {
                            "mihomo · loopback"
                        } else {
                            "core not started"
                        },
                    ))
                    .child(metric_card(
                        theme,
                        "Download",
                        &format_bps(self.down_bps),
                        "live mock",
                    ))
                    .child(metric_card(
                        theme,
                        "Upload",
                        &format_bps(self.up_bps),
                        "live mock",
                    ))
                    .child(metric_card(
                        theme,
                        "Mixed port",
                        "127.0.0.1:7890",
                        "HTTP + SOCKS5",
                    )),
            )
            .child(
                h_flex()
                    .gap_3()
                    .items_start()
                    .child(panel(
                        theme,
                        "Subscriptions",
                        v_flex()
                            .gap_2()
                            .child(row_line(theme, "NekoNet", "48 kept · 3 excluded"))
                            .child(row_line(theme, "Sakura", "22 kept · 1 excluded")),
                    ))
                    .child(panel(
                        theme,
                        "Active groups",
                        v_flex()
                            .gap_2()
                            .child(row_line(theme, "PROXY", "select · 64 nodes"))
                            .child(row_line(theme, "AUTO", "url-test · 64 nodes"))
                            .child(row_line(theme, "Japan", "filter 日|JP · 12 nodes")),
                    )),
            )
    }

    fn subscriptions(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_5()
            .child(page_title(
                theme,
                "Subscriptions",
                "Two airport links. Nodes matching the exclude filter never enter groups.",
            ))
            .child(sub_card(
                theme,
                "NekoNet",
                "https://example.invalid/neko",
                51,
                48,
                "剩余流量 128GB · 套餐到期 2099-01-01 · 官网",
            ))
            .child(sub_card(
                theme,
                "Sakura",
                "https://example.invalid/sakura",
                23,
                22,
                "流量重置日",
            ))
    }

    fn groups(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_5()
            .child(page_title(
                theme,
                "Node Groups",
                "Membership is filter ∪ explicit − exclude. Compiled to mihomo later.",
            ))
            .child(group_card(
                theme,
                "PROXY",
                "select",
                "All kept nodes from both subscriptions",
                &["NekoNet · 东京 01", "NekoNet · 新加坡 03", "Sakura · 大阪 IEPL"],
            ))
            .child(group_card(
                theme,
                "Japan",
                "select · filter (?i)日|jp|tokyo|osaka",
                "12 nodes after filter",
                &["NekoNet · 东京 01", "NekoNet · 东京 02", "Sakura · 大阪 IEPL"],
            ))
    }

    fn routing(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_5()
            .child(page_title(
                theme,
                "App Routing",
                "Apps need a Network Extension. Domains compile to mihomo rules. Demo list only.",
            ))
            .child(panel(
                theme,
                "Rules · first match wins",
                v_flex()
                    .gap_2()
                    .child(rule_line(theme, "Arc.app", "Japan"))
                    .child(rule_line(theme, "Cursor", "PROXY"))
                    .child(rule_line(theme, "*.apple.com", "Direct"))
                    .child(rule_line(theme, "chatgpt.com", "PROXY"))
                    .child(rule_line(theme, "Unmatched apps", "Direct")),
            ))
    }

    fn connections(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_5()
            .child(page_title(
                theme,
                "Connections",
                "Fake live table. Real data comes from the mihomo stream later.",
            ))
            .child(panel(
                theme,
                "Active",
                v_flex()
                    .gap_2()
                    .child(conn_line(theme, "Cursor", "api2.cursor.sh", "PROXY"))
                    .child(conn_line(theme, "Arc", "www.google.com", "Japan"))
                    .child(conn_line(theme, "Music", "amp-api.music.apple.com", "Direct"))
                    .child(conn_line(theme, "ChatGPT", "chatgpt.com", "PROXY")),
            ))
    }

    fn settings(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_5()
            .child(page_title(
                theme,
                "Settings",
                "These values will be written into the strategy document.",
            ))
            .child(panel(
                theme,
                "Inbound",
                v_flex()
                    .gap_2()
                    .child(row_line(theme, "Mixed port", "7890 · HTTP + SOCKS5"))
                    .child(row_line(theme, "Bind", "127.0.0.1"))
                    .child(row_line(theme, "System proxy", "off in this demo")),
            ))
            .child(panel(
                theme,
                "Core",
                v_flex()
                    .gap_2()
                    .child(row_line(theme, "Engine", "mihomo Alpha · not bundled yet"))
                    .child(row_line(theme, "Controller", "loopback + ephemeral secret")),
            ))
    }
}

fn page_title(theme: &Theme, title: &str, subtitle: &str) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_lg().font_semibold().child(title.to_string()))
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(subtitle.to_string()),
        )
}

fn metric_card(theme: &Theme, label: &str, value: &str, hint: &str) -> impl IntoElement {
    v_flex()
        .flex_1()
        .p_4()
        .gap_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label.to_string()),
        )
        .child(div().text_lg().font_semibold().child(value.to_string()))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(hint.to_string()),
        )
}

fn panel(theme: &Theme, title: &str, body: impl IntoElement) -> impl IntoElement {
    v_flex()
        .flex_1()
        .p_4()
        .gap_3()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .child(div().text_sm().font_semibold().child(title.to_string()))
        .child(body)
}

fn row_line(theme: &Theme, left: &str, right: &str) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(div().text_sm().child(left.to_string()))
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(right.to_string()),
        )
}

fn sub_card(
    theme: &Theme,
    name: &str,
    url: &str,
    fetched: u32,
    kept: u32,
    excluded: &str,
) -> impl IntoElement {
    v_flex()
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .child(
            h_flex()
                .justify_between()
                .child(div().text_sm().font_semibold().child(name.to_string()))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("{kept} / {fetched} nodes")),
                ),
        )
        .child(
            div()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(url.to_string()),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("excluded: {excluded}")),
        )
}

fn group_card(
    theme: &Theme,
    name: &str,
    kind: &str,
    summary: &str,
    members: &[&str],
) -> impl IntoElement {
    v_flex()
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_sm().font_semibold().child(name.to_string()))
                .child(pill(theme, kind, theme.accent)),
        )
        .child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(summary.to_string()),
        )
        .children(members.iter().map(|member| {
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(IconName::HardDrive).small())
                .child(div().text_sm().child((*member).to_string()))
        }))
}

fn rule_line(theme: &Theme, source: &str, target: &str) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .child(div().text_sm().child(source.to_string()))
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Icon::new(IconName::ArrowRight)
                        .small()
                        .text_color(theme.muted_foreground),
                )
                .child(div().text_sm().font_semibold().child(target.to_string())),
        )
}

fn conn_line(theme: &Theme, app: &str, dest: &str, via: &str) -> impl IntoElement {
    h_flex()
        .w_full()
        .items_center()
        .gap_3()
        .child(div().w(px(88.)).text_sm().child(app.to_string()))
        .child(
            div()
                .flex_1()
                .text_xs()
                .font_family(theme.mono_font_family.clone())
                .text_color(theme.muted_foreground)
                .child(dest.to_string()),
        )
        .child(pill(theme, via, theme.accent))
}

fn pill(_theme: &Theme, text: &str, color: Hsla) -> impl IntoElement {
    div()
        .px_2()
        .py(px(2.))
        .rounded(px(999.))
        .bg(color.opacity(0.16))
        .text_color(color)
        .text_xs()
        .child(text.to_string())
}

fn status_dot(theme: &Theme, on: bool) -> impl IntoElement {
    let color = if on {
        theme.success
    } else {
        theme.muted_foreground
    };
    div().size_2().rounded_full().bg(color)
}

fn format_bps(bps: f64) -> String {
    if bps < 1_000.0 {
        format!("{bps:.0} B/s")
    } else if bps < 1_000_000.0 {
        format!("{:.1} KB/s", bps / 1_000.0)
    } else {
        format!("{:.2} MB/s", bps / 1_000_000.0)
    }
}
