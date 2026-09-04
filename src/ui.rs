use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme, IconName, Sizable, StyledExt, Theme, TitleBar,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use myproxy::catalog::{self, Catalog};
use myproxy::strategy::{parse_list, Group, Rule, Strategy};
use myproxy::supervisor::Supervisor;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Subscriptions,
    Groups,
    Rules,
    Settings,
}

fn initial_page() -> Page {
    match std::env::var("MYPROXY_PAGE").unwrap_or_default().as_str() {
        "subscriptions" | "subs" => Page::Subscriptions,
        "groups" => Page::Groups,
        "rules" => Page::Rules,
        "settings" => Page::Settings,
        _ => Page::Overview,
    }
}

pub struct AppView {
    page: Page,
    strategy: Strategy,
    catalog: Catalog,
    status: String,
    connected: bool,
    supervisor: Arc<Supervisor>,
    url_input: Entity<InputState>,
    name_input: Entity<InputState>,
    group_name: Entity<InputState>,
    group_sources: Entity<InputState>,
    group_contains: Entity<InputState>,
    rule_match: Entity<InputState>,
    rule_via: Entity<InputState>,
    filter_input: Entity<InputState>,
    port_input: Entity<InputState>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strategy = Strategy::load().unwrap_or_default();
        let catalog = Catalog::load().unwrap_or_default();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(1200))
                .await;
            if this
                .update(cx, |this, cx| {
                    if let Ok(strategy) = Strategy::load() {
                        this.strategy = strategy;
                    }
                    if let Ok(catalog) = Catalog::load() {
                        this.catalog = catalog;
                    }
                    this.connected = this.supervisor.is_running();
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
        .detach();

        let supervisor = Arc::new(Supervisor::default());
        let connected = supervisor.is_running();
        Self {
            page: initial_page(),
            status: "策略已加载。用 CLI 或本页编辑，然后点「应用」或「连接」。".into(),
            connected,
            supervisor,
            url_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder("https://…/clash.yaml")
            }),
            name_input: cx.new(|cx| InputState::new(window, cx).placeholder("订阅名")),
            group_name: cx.new(|cx| InputState::new(window, cx).placeholder("组名")),
            group_sources: cx.new(|cx| {
                InputState::new(window, cx).placeholder("来源：Kitty, Mojie（空=任意订阅）")
            }),
            group_contains: cx.new(|cx| {
                InputState::new(window, cx).placeholder("名称含：jp, tokyo, 日")
            }),
            rule_match: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Arc 或 *.apple.com")
            }),
            rule_via: cx.new(|cx| InputState::new(window, cx).default_value("PROXY")),
            filter_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(strategy.exclude_filter.clone())
            }),
            port_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(strategy.mixed_port.to_string())
            }),
            strategy,
            catalog,
        }
    }

    fn persist(&mut self) {
        match self.strategy.save() {
            Ok(()) => self.status = "已保存策略。".into(),
            Err(err) => self.status = format!("保存失败: {err:#}"),
        }
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

    fn on_apply(&self, cx: &mut Context<Self>) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity();
        move |_, _, app| {
            entity.update(app, |this, cx| {
                match catalog::refresh(&this.strategy).and_then(|cat| {
                    this.catalog = cat.clone();
                    myproxy::compile::compile(&this.strategy, &cat)
                }) {
                    Ok(_) => {
                        this.status = format!(
                            "已编译 {} 个节点，排除 {}。Mixed {}。",
                            this.catalog.nodes.len(),
                            this.catalog.excluded.len(),
                            this.strategy.mixed_port
                        );
                    }
                    Err(err) => this.status = format!("Apply 失败: {err:#}"),
                }
                cx.notify();
            });
        }
    }

    fn on_connect(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity();
        move |_, _, app| {
            entity.update(app, |this, cx| {
                if this.connected {
                    match this.supervisor.disconnect() {
                        Ok(()) => {
                            this.connected = false;
                            this.status = "已断开。".into();
                        }
                        Err(err) => this.status = format!("{err:#}"),
                    }
                } else {
                    match this.supervisor.connect(&this.strategy) {
                        Ok(()) => {
                            this.connected = true;
                            this.catalog = Catalog::load().unwrap_or_default();
                            this.status = format!(
                                "已连接 127.0.0.1:{} （HTTP + SOCKS5）",
                                this.strategy.mixed_port
                            );
                        }
                        Err(err) => this.status = format!("{err:#}"),
                    }
                }
                cx.notify();
            });
        }
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
            toggle.danger().label("断开")
        } else {
            toggle.primary().label("连接")
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
                        .gap_2()
                        .items_center()
                        .child(div().text_sm().font_semibold().child("myproxy"))
                        .child(pill(theme, "DEV", theme.warning)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(status_dot(theme, connected))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(if connected {
                                    format!("已连接 · mixed {}", self.strategy.mixed_port)
                                } else {
                                    "未连接".into()
                                }),
                        )
                        .child(
                            Button::new("apply")
                                .small()
                                .label("应用")
                                .on_click(self.on_apply(cx)),
                        )
                        .child(toggle.on_click(self.on_connect(cx))),
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
                        .child(div().text_sm().font_semibold().child("控制"))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("strategy.json"),
                        ),
                ),
            )
            .child(
                SidebarGroup::new("会话").child(
                    SidebarMenu::new()
                        .child(self.nav_item(cx, Page::Overview, "总览", IconName::LayoutDashboard))
                        .child(self.nav_item(cx, Page::Subscriptions, "订阅", IconName::Inbox))
                        .child(self.nav_item(cx, Page::Groups, "节点组", IconName::Folder))
                        .child(self.nav_item(cx, Page::Rules, "规则", IconName::Map)),
                ),
            )
            .child(
                SidebarGroup::new("系统").child(
                    SidebarMenu::new()
                        .child(self.nav_item(cx, Page::Settings, "设置", IconName::Settings)),
                ),
            )
            .footer(
                SidebarFooter::new().child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "{} nodes · {} excluded",
                            self.catalog.nodes.len(),
                            self.catalog.excluded.len()
                        )),
                ),
            )
    }

    fn page_view(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        v_flex()
            .id("page")
            .flex_1()
            .h_full()
            .overflow_hidden()
            .p_6()
            .gap_4()
            .bg(theme.background)
            .child(
                div()
                    .id("status")
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.status.clone()),
            )
            .child(
                div()
                    .id("page-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(match self.page {
                        Page::Overview => self.overview(theme).into_any_element(),
                        Page::Subscriptions => self.subscriptions(cx, theme).into_any_element(),
                        Page::Groups => self.groups(cx, theme).into_any_element(),
                        Page::Rules => self.rules(cx, theme).into_any_element(),
                        Page::Settings => self.settings(cx, theme).into_any_element(),
                    }),
            )
    }

    fn overview(&self, theme: &Theme) -> impl IntoElement {
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "总览",
                "策略文档是权威源。连接会启动捆绑的 mihomo。",
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(metric(
                        theme,
                        "Status",
                        if self.connected { "已连接" } else { "空闲" },
                    ))
                    .child(metric(
                        theme,
                        "Mixed port",
                        &format!("127.0.0.1:{}", self.strategy.mixed_port),
                    ))
                    .child(metric(
                        theme,
                        "Nodes",
                        &format!(
                            "{} kept / {} excluded",
                            self.catalog.nodes.len(),
                            self.catalog.excluded.len()
                        ),
                    ))
                    .child(metric(
                        theme,
                        "Rules",
                        &self.strategy.rules.len().to_string(),
                    )),
            )
    }

    fn subscriptions(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "订阅",
                "多个机场 URL。排除过滤器在导入时丢掉流量/余额/官网一类节点。",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(160.)).child(Input::new(&self.name_input)))
                    .child(div().flex_1().child(Input::new(&self.url_input)))
                    .child({
                        let entity = entity.clone();
                        Button::new("add-sub").primary().label("添加").on_click(
                            move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    let name = this.name_input.read(cx).value().to_string();
                                    let url = this.url_input.read(cx).value().to_string();
                                    if url.trim().is_empty() {
                                        this.status = "需要订阅 URL。".into();
                                    } else {
                                        let name = if name.trim().is_empty() {
                                            "sub".into()
                                        } else {
                                            name
                                        };
                                        this.strategy.add_subscription(name, url.trim().into());
                                        this.persist();
                                        this.name_input.update(cx, |input, cx| {
                                            input.set_value("", window, cx);
                                        });
                                        this.url_input.update(cx, |input, cx| {
                                            input.set_value("", window, cx);
                                        });
                                    }
                                    cx.notify();
                                });
                            },
                        )
                    }),
            )
            .when(self.strategy.subscriptions.is_empty(), |this| {
                this.child(empty_hint(
                    theme,
                    "还没有订阅。填 URL 后点添加，或用 myproxyctl subscription add。",
                ))
            })
            .children(self.strategy.subscriptions.iter().map(|sub| {
                let id = sub.id.clone();
                let entity = entity.clone();
                panel(
                    theme,
                    &sub.name,
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(sub.url.clone()),
                        )
                        .child(
                            Button::new(SharedString::from(format!("del-{id}")))
                                .small()
                                .danger()
                                .label("删除")
                                .on_click(move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.strategy.remove_subscription(&id);
                                        this.persist();
                                        cx.notify();
                                    });
                                }),
                        ),
                )
            }))
            .children(self.catalog.excluded.iter().take(8).map(|ex| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("excluded {} ({}) — {}", ex.name, ex.subscription, ex.reason))
            }))
    }

    fn groups(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "节点组",
                "条件组：来源 ∩ 名称包含（多条为或）∪ 钉住 − 排除。空条件不会再变成「全部节点」。PROXY 用「全部」。",
            ))
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().w(px(140.)).child(Input::new(&self.group_name)))
                            .child(div().flex_1().child(Input::new(&self.group_sources))),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().child(Input::new(&self.group_contains)))
                            .child({
                                let entity = entity.clone();
                                Button::new("add-group").primary().label("添加条件组").on_click(
                                    move |_, window, app| {
                                        entity.update(app, |this, cx| {
                                            let name = this.group_name.read(cx).value().to_string();
                                            let sources =
                                                parse_list(&this.group_sources.read(cx).value());
                                            let contains =
                                                parse_list(&this.group_contains.read(cx).value());
                                            if name.trim().is_empty() {
                                                this.status = "需要组名。".into();
                                            } else if sources.is_empty() && contains.is_empty() {
                                                this.status =
                                                    "条件组需要来源或名称包含；否则请用「添加全部」。"
                                                        .into();
                                            } else {
                                                this.strategy.add_group(Group::matching(
                                                    name.trim().into(),
                                                    "select".into(),
                                                    sources,
                                                    contains,
                                                ));
                                                this.persist();
                                                this.group_name.update(cx, |i, cx| {
                                                    i.set_value("", window, cx)
                                                });
                                            }
                                            cx.notify();
                                        });
                                    },
                                )
                            })
                            .child({
                                let entity = entity.clone();
                                Button::new("add-group-all").label("添加全部").on_click(
                                    move |_, window, app| {
                                        entity.update(app, |this, cx| {
                                            let name = this.group_name.read(cx).value().to_string();
                                            let sources =
                                                parse_list(&this.group_sources.read(cx).value());
                                            if name.trim().is_empty() {
                                                this.status = "需要组名。".into();
                                            } else {
                                                let mut group = Group::all_nodes(
                                                    name.trim().into(),
                                                    "select".into(),
                                                );
                                                group.sources = sources;
                                                this.strategy.add_group(group);
                                                this.persist();
                                                this.group_name.update(cx, |i, cx| {
                                                    i.set_value("", window, cx)
                                                });
                                            }
                                            cx.notify();
                                        });
                                    },
                                )
                            }),
                    ),
            )
            .when(self.strategy.groups.is_empty(), |this| {
                this.child(empty_hint(theme, "还没有节点组。默认会有 PROXY 组。"))
            })
            .children(self.strategy.groups.iter().map(|group| {
                let members = catalog::resolve_group_members(group, &self.catalog);
                let preview = members.iter().take(6).cloned().collect::<Vec<_>>().join(" · ");
                let id = group.id.clone();
                let entity = entity.clone();
                panel(
                    theme,
                    &format!("{}  ·  {}  ·  {} 个节点", group.name, group.kind, members.len()),
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(group.policy_label()),
                        )
                        .child(
                            div().text_xs().child(if preview.is_empty() {
                                "没有成员。条件为空就不会自动进组；先 Apply 订阅或钉住节点。"
                                    .into()
                            } else {
                                preview
                            }),
                        )
                        .child(
                            h_flex().justify_end().child(
                                Button::new(SharedString::from(format!("del-group-{id}")))
                                    .small()
                                    .danger()
                                    .label("删除")
                                    .on_click(move |_, _, app| {
                                        entity.update(app, |this, cx| {
                                            this.strategy.remove_group(&id);
                                            this.persist();
                                            cx.notify();
                                        });
                                    }),
                            ),
                        ),
                )
            }))
    }

    fn rules(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "规则",
                "按应用或域名选择节点 / 节点组 / Direct / Reject。自上而下第一条命中。应用规则编译为进程名，需流量进入 Mixed 口。",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.rule_match)))
                    .child(div().w(px(140.)).child(Input::new(&self.rule_via)))
                    .child({
                        let entity = entity.clone();
                        Button::new("add-rule-app").label("作应用").on_click(
                            move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    let m = this.rule_match.read(cx).value().to_string();
                                    let via = this.rule_via.read(cx).value().to_string();
                                    if m.trim().is_empty() {
                                        this.status = "填写应用名，例如 Arc。".into();
                                    } else {
                                        this.strategy.add_rule(Rule::new_app(m.trim().into(), via));
                                        this.persist();
                                        this.rule_match.update(cx, |i, cx| i.set_value("", window, cx));
                                    }
                                    cx.notify();
                                });
                            },
                        )
                    })
                    .child({
                        let entity = entity.clone();
                        Button::new("add-rule-domain").primary().label("作域名").on_click(
                            move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    let m = this.rule_match.read(cx).value().to_string();
                                    let via = this.rule_via.read(cx).value().to_string();
                                    if m.trim().is_empty() {
                                        this.status = "填写域名或 *.suffix。".into();
                                    } else if m.contains('*') || m.starts_with('.') {
                                        this.strategy.add_rule(Rule::new_suffix(
                                            m.trim().trim_start_matches("*.").into(),
                                            via,
                                        ));
                                        this.persist();
                                        this.rule_match.update(cx, |i, cx| i.set_value("", window, cx));
                                    } else {
                                        this.strategy.add_rule(Rule::new_domain(m.trim().into(), via));
                                        this.persist();
                                        this.rule_match.update(cx, |i, cx| i.set_value("", window, cx));
                                    }
                                    cx.notify();
                                });
                            },
                        )
                    }),
            )
            .when(self.strategy.rules.is_empty(), |this| {
                this.child(empty_hint(
                    theme,
                    "还没有规则。未匹配的流量走 PROXY（或第一组）。应用规则目前是进程名，不是系统级拦截。",
                ))
            })
            .children(self.strategy.rules.iter().map(|rule| {
                let id = rule.id.clone();
                let entity = entity.clone();
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.group_box)
                    .child(div().text_sm().child(rule.match_label()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(pill(theme, &rule.via, theme.accent))
                            .child(
                                Button::new(SharedString::from(format!("del-rule-{id}")))
                                    .small()
                                    .danger()
                                    .label("删除")
                                    .on_click(move |_, _, app| {
                                        entity.update(app, |this, cx| {
                                            this.strategy.remove_rule(&id);
                                            this.persist();
                                            cx.notify();
                                        });
                                    }),
                            ),
                    )
            }))
    }

    fn settings(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "设置",
                "Mixed 一口同时提供 HTTP 代理与 SOCKS5。排除器用正则。",
            ))
            .child(panel(
                theme,
                "入口",
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("127.0.0.1"))
                    .child(div().w(px(100.)).child(Input::new(&self.port_input)))
                    .child({
                        let entity = entity.clone();
                        Button::new("save-port").label("保存端口").on_click(
                            move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    if let Ok(port) = this.port_input.read(cx).value().parse::<u16>()
                                    {
                                        this.strategy.mixed_port = port;
                                        this.persist();
                                    } else {
                                        this.status = "端口无效。".into();
                                    }
                                    cx.notify();
                                });
                            },
                        )
                    }),
            ))
            .child(panel(
                theme,
                "排除过滤器",
                v_flex()
                    .gap_2()
                    .child(Input::new(&self.filter_input))
                    .child({
                        let entity = entity.clone();
                        Button::new("save-filter").primary().label("保存过滤器").on_click(
                            move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    this.strategy.exclude_filter =
                                        this.filter_input.read(cx).value().to_string();
                                    this.persist();
                                    cx.notify();
                                });
                            },
                        )
                    }),
            ))
    }
}

fn empty_hint(theme: &Theme, text: &str) -> impl IntoElement {
    div()
        .p_4()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(text.to_string())
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

fn metric(theme: &Theme, label: &str, value: &str) -> impl IntoElement {
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
        .child(div().text_sm().font_semibold().child(value.to_string()))
}

fn panel(theme: &Theme, title: &str, body: impl IntoElement) -> impl IntoElement {
    v_flex()
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .child(div().text_sm().font_semibold().child(title.to_string()))
        .child(body)
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
    div()
        .size_2()
        .rounded_full()
        .bg(if on {
            theme.success
        } else {
            theme.muted_foreground
        })
}
