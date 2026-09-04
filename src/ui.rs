use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme, IconName, Selectable, Sizable, StyledExt, Theme, TitleBar,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use myproxy::catalog::{self, Catalog};
use myproxy::strategy::{parse_list, Group, Rule, Strategy};
use myproxy::supervisor::Supervisor;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleDraftKind {
    App,
    Exact,
    Suffix,
    Keyword,
}

impl RuleDraftKind {
    const ALL: [Self; 4] = [Self::App, Self::Exact, Self::Suffix, Self::Keyword];

    fn from_rule(rule: &Rule) -> Self {
        if !rule.app.is_empty() {
            Self::App
        } else if !rule.keyword.is_empty() {
            Self::Keyword
        } else if !rule.domain.is_empty() {
            Self::Exact
        } else {
            Self::Suffix
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::App => "进程",
            Self::Exact => "域名",
            Self::Suffix => "后缀",
            Self::Keyword => "关键字",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::App => "进程名，例如 Arc",
            Self::Exact => "apple.com",
            Self::Suffix => "apple.com，匹配其子域",
            Self::Keyword => "关键字，例如 google",
        }
    }

    fn into_rule(self, match_value: String, via: String) -> Rule {
        match self {
            Self::App => Rule::new_app(match_value, via),
            Self::Exact => Rule::new_domain(match_value, via),
            Self::Suffix => Rule::new_suffix(normalize_suffix(&match_value), via),
            Self::Keyword => Rule::new_keyword(match_value, via),
        }
    }
}

fn normalize_suffix(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .to_string()
}

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
    rule_draft_kind: RuleDraftKind,
    rule_edit_id: Option<String>,
    url_input: Entity<InputState>,
    name_input: Entity<InputState>,
    group_name: Entity<InputState>,
    group_sources: Entity<InputState>,
    group_contains: Entity<InputState>,
    rule_match: Entity<InputState>,
    rule_via: Entity<InputState>,
    rule_query: Entity<InputState>,
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
            rule_draft_kind: RuleDraftKind::Suffix,
            rule_edit_id: None,
            rule_match: cx.new(|cx| {
                InputState::new(window, cx).placeholder(RuleDraftKind::Suffix.placeholder())
            }),
            rule_via: cx.new(|cx| InputState::new(window, cx).default_value("PROXY")),
            rule_query: cx.new(|cx| InputState::new(window, cx).placeholder("筛选规则…")),
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

    fn persist(&mut self) -> bool {
        match self.strategy.save() {
            Ok(()) => {
                self.status = "已保存策略。".into();
                true
            }
            Err(err) => {
                self.status = format!("保存失败: {err:#}");
                false
            }
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

    fn set_rule_kind(&mut self, kind: RuleDraftKind, window: &mut Window, cx: &mut Context<Self>) {
        self.rule_draft_kind = kind;
        self.rule_match.update(cx, |input, cx| {
            input.set_placeholder(kind.placeholder(), window, cx);
        });
    }

    fn begin_edit_rule(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self.strategy.rules.iter().find(|r| r.id == id).cloned() else {
            self.status = "找不到这条规则。".into();
            return;
        };
        self.rule_edit_id = Some(id.to_string());
        self.set_rule_kind(RuleDraftKind::from_rule(&rule), window, cx);
        let match_value = rule.match_value().to_string();
        let via = rule.via.clone();
        self.rule_match.update(cx, |input, cx| {
            input.set_value(match_value, window, cx);
        });
        self.rule_via.update(cx, |input, cx| {
            input.set_value(via, window, cx);
        });
        self.status = format!("正在编辑 {} → {}", rule.match_label(), rule.via);
    }

    fn cancel_edit_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rule_edit_id = None;
        self.rule_match.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(self.rule_draft_kind.placeholder(), window, cx);
        });
        self.rule_via.update(cx, |input, cx| {
            input.set_value("PROXY", window, cx);
        });
        self.status = "已取消编辑。".into();
    }

    fn commit_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let match_value = self.rule_match.read(cx).value().trim().to_string();
        let via = self.rule_via.read(cx).value().trim().to_string();
        if match_value.is_empty() {
            self.status = format!("填写{}。", self.rule_draft_kind.placeholder());
            return;
        }
        if via.is_empty() {
            self.status = "填写走向，例如 PROXY、DIRECT、REJECT。".into();
            return;
        }
        let next = self.rule_draft_kind.into_rule(match_value, via);
        if let Some(id) = self.rule_edit_id.clone() {
            if !self.strategy.update_rule(&id, next) {
                self.status = "保存失败：规则已不存在。".into();
                return;
            }
            if self.persist() {
                self.cancel_edit_rule(window, cx);
                self.status = "已保存规则。".into();
            }
        } else {
            self.strategy.add_rule(next);
            if self.persist() {
                self.rule_match.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
                self.status = "已添加规则。".into();
            }
        }
    }

    fn set_selected_rule_via(&mut self, id: &str, via: &str) {
        if !self.strategy.set_rule_via(id, via.to_string()) {
            self.status = "改走向失败。".into();
            return;
        }
        if self.persist() {
            self.status = format!("已改为 {via}。");
        }
    }

    fn move_selected_rule(&mut self, id: &str, delta: i32) {
        if !self.strategy.move_rule(id, delta) {
            return;
        }
        if self.persist() {
            self.status = if delta < 0 {
                "已上移，优先级提高。".into()
            } else {
                "已下移，优先级降低。".into()
            };
        }
    }

    fn remove_selected_rule(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editing = self.rule_edit_id.as_deref() == Some(id);
        if !self.strategy.remove_rule(id) {
            return;
        }
        if self.persist() {
            if editing {
                self.rule_edit_id = None;
                self.rule_match.update(cx, |input, cx| {
                    input.set_value("", window, cx);
                });
            }
            self.status = "已删除规则。".into();
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
                                .label("更新配置")
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
            .child(match self.page {
                Page::Rules => self.rules(cx, theme).into_any_element(),
                page => div()
                    .id("page-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(match page {
                        Page::Overview => self.overview(theme).into_any_element(),
                        Page::Subscriptions => self.subscriptions(cx, theme).into_any_element(),
                        Page::Groups => self.groups(cx, theme).into_any_element(),
                        Page::Settings => self.settings(cx, theme).into_any_element(),
                        Page::Rules => unreachable!(),
                    })
                    .into_any_element(),
            })
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
        let editing = self.rule_edit_id.is_some();
        let composer_title = if let Some(id) = &self.rule_edit_id {
            match self.strategy.rules.iter().position(|r| &r.id == id) {
                Some(index) => format!("编辑第 {} 条", index + 1),
                None => "编辑规则".into(),
            }
        } else {
            "添加规则".into()
        };
        let query = self.rule_query.read(cx).value().to_string();
        let total = self.strategy.rules.len();
        let visible: Vec<(usize, Rule)> = self
            .strategy
            .rules
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, rule)| rule.matches_query(&query))
            .collect();
        let visible_len = visible.len();
        let muted = theme.muted;
        let muted_fg = theme.muted_foreground;
        let border = theme.border;
        let radius = theme.radius;
        let group_box = theme.group_box;

        v_flex()
            .id("rules-page")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .gap_4()
            .child(page_title(
                theme,
                "规则",
                "自上而下第一条命中。单击一行编辑；右键打开菜单。应用规则是进程名，流量要进 Mixed 口。",
            ))
            .child(panel(
                theme,
                &composer_title,
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child("类型"),
                            )
                            .child({
                                let entity = entity.clone();
                                let mut group = ButtonGroup::new("rule-kind")
                                    .compact()
                                    .outline()
                                    .small();
                                for kind in RuleDraftKind::ALL {
                                    group = group.child(
                                        Button::new(SharedString::from(format!(
                                            "rule-kind-{}",
                                            kind.label()
                                        )))
                                        .small()
                                        .label(kind.label())
                                        .selected(self.rule_draft_kind == kind),
                                    );
                                }
                                group.on_click(move |ixs, window, app| {
                                    let Some(&ix) = ixs.first() else {
                                        return;
                                    };
                                    let Some(kind) = RuleDraftKind::ALL.get(ix).copied() else {
                                        return;
                                    };
                                    entity.update(app, |this, cx| {
                                        this.set_rule_kind(kind, window, cx);
                                        cx.notify();
                                    });
                                })
                            }),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().child(Input::new(&self.rule_match)))
                            .child(div().w(px(140.)).child(Input::new(&self.rule_via)))
                            .child({
                                let entity = entity.clone();
                                Button::new("commit-rule")
                                    .small()
                                    .primary()
                                    .label(if editing { "保存" } else { "添加" })
                                    .on_click(move |_, window, app| {
                                        entity.update(app, |this, cx| {
                                            this.commit_rule(window, cx);
                                            cx.notify();
                                        });
                                    })
                            })
                            .when(editing, |this| {
                                let entity = entity.clone();
                                this.child(
                                    Button::new("cancel-rule").small().label("取消").on_click(
                                        move |_, window, app| {
                                            entity.update(app, |this, cx| {
                                                this.cancel_edit_rule(window, cx);
                                                cx.notify();
                                            });
                                        },
                                    ),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child("走向填 PROXY、DIRECT、REJECT 或节点组名。"),
                    ),
            ))
            .child(
                v_flex()
                    .id("rule-table")
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .gap_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().flex_1().child(Input::new(&self.rule_query)))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(if query.trim().is_empty() {
                                        format!("{total} 条")
                                    } else {
                                        format!("{visible_len} / {total} 条")
                                    }),
                            ),
                    )
                    .child(
                        v_flex()
                            .id("rule-list")
                            .flex_1()
                            .min_h_0()
                            .overflow_hidden()
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .bg(group_box)
                            .child(
                                rule_columns(
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child("#"),
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child("类型"),
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child("匹配"),
                                    div()
                                        .text_xs()
                                        .text_color(muted_fg)
                                        .child("走向"),
                                )
                                .px_3()
                                .py_2()
                                .bg(muted),
                            )
                            .child(
                                v_flex()
                                    .id("rules-scroll")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .when(self.strategy.rules.is_empty(), |this| {
                                        this.child(
                                            div()
                                                .p_4()
                                                .text_sm()
                                                .text_color(muted_fg)
                                                .child("还没有规则。选类型、填匹配和走向，再点添加。未匹配的流量走 PROXY。".to_string()),
                                        )
                                    })
                                    .when(
                                        !self.strategy.rules.is_empty() && visible.is_empty(),
                                        |this| {
                                            this.child(
                                                div()
                                                    .p_4()
                                                    .text_sm()
                                                    .text_color(muted_fg)
                                                    .child("没有匹配筛选的规则。".to_string()),
                                            )
                                        },
                                    )
                                    .children(visible.into_iter().map(|(index, rule)| {
                                        render_rule_row(
                                            entity.clone(),
                                            theme,
                                            index,
                                            total,
                                            self.rule_edit_id.as_deref() == Some(rule.id.as_str()),
                                            &rule,
                                        )
                                    })),
                            ),
                    ),
            )
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

fn rule_columns(
    index: impl IntoElement,
    kind: impl IntoElement,
    match_el: impl IntoElement,
    via: impl IntoElement,
) -> Div {
    h_flex()
        .w_full()
        .items_center()
        .gap(px(12.))
        .child(div().w(px(36.)).flex_shrink_0().child(index))
        .child(div().w(px(64.)).flex_shrink_0().child(kind))
        .child(div().flex_1().min_w(px(0.)).child(match_el))
        .child(div().w(px(120.)).flex_shrink_0().child(via))
}

fn outline_pill(theme: &Theme, text: &str) -> impl IntoElement {
    div()
        .px_2()
        .py(px(2.))
        .rounded(px(6.))
        .border_1()
        .border_color(theme.border)
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(text.to_string())
}

fn render_rule_row(
    entity: Entity<AppView>,
    theme: &Theme,
    index: usize,
    total: usize,
    selected: bool,
    rule: &Rule,
) -> impl IntoElement {
    let id = rule.id.clone();
    let via = rule.via.clone();
    let can_up = index > 0;
    let can_down = index + 1 < total;
    let muted = theme.muted;
    let muted_fg = theme.muted_foreground;
    let accent = theme.accent;
    let mono = theme.mono_font_family.clone();
    rule_columns(
        div()
            .text_xs()
            .font_family(mono)
            .text_color(muted_fg)
            .child(format!("{}", index + 1)),
        outline_pill(theme, rule.kind_label()),
        div().text_sm().child(rule.match_value().to_string()),
        pill(theme, &via, accent),
    )
    .id(SharedString::from(format!("rule-{id}")))
    .px_3()
    .py_2()
    .cursor_pointer()
    .when(selected, |this| this.bg(accent.opacity(0.14)))
    .hover(move |style| style.bg(muted))
    .on_click({
        let entity = entity.clone();
        let id = id.clone();
        move |_, window, app| {
            entity.update(app, |this, cx| {
                this.begin_edit_rule(&id, window, cx);
                cx.notify();
            });
        }
    })
    .context_menu(move |menu, window, cx| {
        let edit_id = id.clone();
        let edit_entity = entity.clone();
        let up_id = id.clone();
        let up_entity = entity.clone();
        let down_id = id.clone();
        let down_entity = entity.clone();
        let del_id = id.clone();
        let del_entity = entity.clone();
        menu.min_w(px(168.))
            .item(PopupMenuItem::new("编辑").on_click(move |_, window, app| {
                edit_entity.update(app, |this, cx| {
                    this.begin_edit_rule(&edit_id, window, cx);
                    cx.notify();
                });
            }))
            .separator()
            .item(
                PopupMenuItem::new("上移")
                    .disabled(!can_up)
                    .on_click(move |_, _, app| {
                        up_entity.update(app, |this, cx| {
                            this.move_selected_rule(&up_id, -1);
                            cx.notify();
                        });
                    }),
            )
            .item(
                PopupMenuItem::new("下移")
                    .disabled(!can_down)
                    .on_click(move |_, _, app| {
                        down_entity.update(app, |this, cx| {
                            this.move_selected_rule(&down_id, 1);
                            cx.notify();
                        });
                    }),
            )
            .separator()
            .submenu("改为走向", window, cx, {
                let entity = entity.clone();
                let id = id.clone();
                let via_current = via.clone();
                move |menu, _, _| {
                    ["DIRECT", "PROXY", "REJECT"].into_iter().fold(menu, |menu, target| {
                        let entity = entity.clone();
                        let id = id.clone();
                        menu.item(
                            PopupMenuItem::new(target)
                                .checked(via_current.eq_ignore_ascii_case(target))
                                .on_click(move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.set_selected_rule_via(&id, target);
                                        cx.notify();
                                    });
                                }),
                        )
                    })
                }
            })
            .separator()
            .item(PopupMenuItem::new("删除").on_click(move |_, window, app| {
                del_entity.update(app, |this, cx| {
                    this.remove_selected_rule(&del_id, window, cx);
                    cx.notify();
                });
            }))
    })
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
