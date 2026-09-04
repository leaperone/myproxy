use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::button::{Button, ButtonGroup, ButtonVariants as _};
use gpui_kit::component::dialog::DialogButtonProps;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenuItem};
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarGroup, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme, IconName, Root, Selectable, Sizable, StyledExt, Theme, TitleBar,
    WindowExt,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use myproxy::catalog::{self, Catalog};
use myproxy::log;
use myproxy::strategy::{join_list, parse_list, Group, Rule, Strategy};
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

#[derive(Clone)]
struct ViaChoice {
    value: String,
    label: String,
}

const REGION_PRESETS: &[(&str, &[&str])] = &[
    ("JP", &["jp", "日", "tokyo", "东京"]),
    ("US", &["us", "美"]),
    ("HK", &["hk", "港"]),
    ("TW", &["tw", "台"]),
];

struct GroupEditor {
    parent: Entity<AppView>,
    edit_id: Option<String>,
    notice: String,
    all_nodes: bool,
    kind: String,
    name: Entity<InputState>,
    sources: Entity<InputState>,
    contains: Entity<InputState>,
    excludes: Entity<InputState>,
    include: Vec<String>,
    blocked: Vec<String>,
}

impl GroupEditor {
    fn new(
        parent: Entity<AppView>,
        existing: Option<Group>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let edit_id = existing.as_ref().map(|g| g.id.clone());
        let all_nodes = existing.as_ref().map(|g| g.all_nodes).unwrap_or(false);
        let kind = existing
            .as_ref()
            .map(|g| {
                if g.kind == "url-test" {
                    "url-test"
                } else {
                    "select"
                }
            })
            .unwrap_or("select")
            .to_string();
        let include = existing
            .as_ref()
            .map(|g| g.include.clone())
            .unwrap_or_default();
        let blocked = existing
            .as_ref()
            .map(|g| g.exclude.clone())
            .unwrap_or_default();
        let name = existing
            .as_ref()
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let sources = existing
            .as_ref()
            .map(|g| join_list(&g.sources))
            .unwrap_or_default();
        let contains = existing
            .as_ref()
            .map(|g| join_list(&g.name_contains))
            .unwrap_or_default();
        let excludes = existing
            .as_ref()
            .map(|g| join_list(&g.name_excludes))
            .unwrap_or_default();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("组名")
                .default_value(name)
        });
        let sources = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("来源：空=任意订阅")
                .default_value(sources)
        });
        let contains = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("名称含：jp, tokyo, 日 或 JP*")
                .default_value(contains)
        });
        let excludes = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("名称不含：IEPL, x0.1")
                .default_value(excludes)
        });
        cx.observe(&sources, |_, _, cx| cx.notify()).detach();
        cx.observe(&contains, |_, _, cx| cx.notify()).detach();
        cx.observe(&excludes, |_, _, cx| cx.notify()).detach();
        Self {
            parent,
            edit_id,
            notice: String::new(),
            all_nodes,
            kind,
            name,
            sources,
            contains,
            excludes,
            include,
            blocked,
        }
    }

    fn set_list_value(
        input: &Entity<InputState>,
        parts: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = join_list(parts);
        input.update(cx, |i, cx| {
            i.set_value(text, window, cx);
        });
    }

    fn draft(&self, cx: &App) -> Group {
        Group {
            id: self.edit_id.clone().unwrap_or_default(),
            name: self.name.read(cx).value().trim().to_string(),
            kind: self.kind.clone(),
            all_nodes: self.all_nodes,
            sources: parse_list(&self.sources.read(cx).value()),
            name_contains: parse_list(&self.contains.read(cx).value()),
            name_excludes: parse_list(&self.excludes.read(cx).value()),
            include: self.include.clone(),
            exclude: self.blocked.clone(),
            filter: String::new(),
        }
    }

    fn commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut next = self.draft(cx);
        if next.name.is_empty() {
            self.notice = "需要组名。".into();
            cx.notify();
            return false;
        }
        if !next.all_nodes
            && next.sources.is_empty()
            && next.name_contains.is_empty()
            && next.include.is_empty()
        {
            self.notice = "条件组需要来源、名称含或钉住；否则打开「全部」。".into();
            cx.notify();
            return false;
        }
        let edit_id = self.edit_id.clone();
        let result = self.parent.update(cx, |parent, cx| {
            if let Some(id) = &edit_id {
                if !parent.strategy.update_group(id, next) {
                    return Err("保存失败：节点组已不存在。".to_string());
                }
            } else {
                next.id = uuid::Uuid::new_v4().to_string();
                parent.strategy.add_group(next);
            }
            if parent.persist() {
                parent.group_modal_open = false;
                parent.group_edit_id = None;
                parent.status = if edit_id.is_some() {
                    "已保存节点组。".into()
                } else {
                    "已添加节点组。".into()
                };
                cx.notify();
                Ok(())
            } else {
                Err(parent.status.clone())
            }
        });
        match result {
            Ok(()) => true,
            Err(msg) => {
                self.notice = msg;
                cx.notify();
                false
            }
        }
    }

    fn toggle_source(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let mut sources = parse_list(&self.sources.read(cx).value());
        if sources.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            sources.retain(|s| !s.eq_ignore_ascii_case(name));
        } else {
            sources.push(name.to_string());
        }
        Self::set_list_value(&self.sources, &sources, window, cx);
    }

    fn remove_token(
        input: &Entity<InputState>,
        token: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut parts = parse_list(&input.read(cx).value());
        parts.retain(|p| p != token);
        Self::set_list_value(input, &parts, window, cx);
    }

    fn append_contains(&mut self, tokens: &[&str], window: &mut Window, cx: &mut Context<Self>) {
        let mut parts = parse_list(&self.contains.read(cx).value());
        for token in tokens {
            if !parts.iter().any(|p| p.eq_ignore_ascii_case(token)) {
                parts.push((*token).to_string());
            }
        }
        self.all_nodes = false;
        Self::set_list_value(&self.contains, &parts, window, cx);
    }

    fn pin_member(&mut self, name: &str) {
        self.blocked.retain(|n| n != name);
        if !self.include.iter().any(|n| n == name) {
            self.include.push(name.to_string());
        }
        self.notice = format!("已钉住 {name}。保存后生效。");
    }

    fn block_member(&mut self, name: &str) {
        self.include.retain(|n| n != name);
        if !self.blocked.iter().any(|n| n == name) {
            self.blocked.push(name.to_string());
        }
        self.notice = format!("已排除 {name}。保存后生效。");
    }

    fn unpin_member(&mut self, name: &str) {
        self.include.retain(|n| n != name);
        self.notice = format!("取消钉住 {name}。");
    }

    fn unblock_member(&mut self, name: &str) {
        self.blocked.retain(|n| n != name);
        self.notice = format!("取消排除 {name}。");
    }
}

impl Render for GroupEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let parent = self.parent.read(cx);
        let theme = cx.theme().clone();
        let draft = self.draft(cx);
        let started = Instant::now();
        let members = catalog::resolve_group_members(&draft, &parent.catalog);
        let resolve_ms = started.elapsed().as_millis();
        if resolve_ms >= 8 {
            log::debug(
                "ui",
                format!(
                    "group preview resolve {} members in {resolve_ms}ms",
                    members.len()
                ),
            );
        }
        const PREVIEW_LIMIT: usize = 80;
        let extra = members.len().saturating_sub(PREVIEW_LIMIT);
        let contains = parse_list(&self.contains.read(cx).value());
        let excludes = parse_list(&self.excludes.read(cx).value());
        let sources = parse_list(&self.sources.read(cx).value());
        let muted_fg = theme.muted_foreground;
        let border = theme.border;
        let radius = theme.radius;
        let group_box = theme.group_box;
        let subscriptions = parent.strategy.subscriptions.clone();

        v_flex()
            .gap_3()
            .when(!self.notice.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(self.notice.clone()),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().w(px(160.)).child(Input::new(&self.name)))
                    .child({
                        let entity = entity.clone();
                        let mut group = ButtonGroup::new("group-mode")
                            .compact()
                            .outline()
                            .small();
                        group = group
                            .child(
                                Button::new("group-mode-match")
                                    .small()
                                    .label("条件")
                                    .selected(!self.all_nodes),
                            )
                            .child(
                                Button::new("group-mode-all")
                                    .small()
                                    .label("全部")
                                    .selected(self.all_nodes),
                            );
                        group.on_click(move |ixs, _, app| {
                            let Some(&ix) = ixs.first() else {
                                return;
                            };
                            entity.update(app, |this, cx| {
                                this.all_nodes = ix == 1;
                                cx.notify();
                            });
                        })
                    })
                    .child({
                        let entity = entity.clone();
                        let mut group = ButtonGroup::new("group-kind")
                            .compact()
                            .outline()
                            .small();
                        group = group
                            .child(
                                Button::new("group-kind-select")
                                    .small()
                                    .label("select")
                                    .selected(self.kind != "url-test"),
                            )
                            .child(
                                Button::new("group-kind-url")
                                    .small()
                                    .label("url-test")
                                    .selected(self.kind == "url-test"),
                            );
                        group.on_click(move |ixs, _, app| {
                            let Some(&ix) = ixs.first() else {
                                return;
                            };
                            entity.update(app, |this, cx| {
                                this.kind = if ix == 1 {
                                    "url-test".into()
                                } else {
                                    "select".into()
                                };
                                cx.notify();
                            });
                        })
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .flex_wrap()
                    .child(div().text_xs().text_color(muted_fg).child("来源"))
                    .child({
                        let entity = entity.clone();
                        Button::new("src-any")
                            .small()
                            .label("任意")
                            .selected(sources.is_empty())
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    GroupEditor::set_list_value(&this.sources, &[], window, cx);
                                    cx.notify();
                                });
                            })
                    })
                    .children(subscriptions.iter().map(|sub| {
                        let entity = entity.clone();
                        let name = sub.name.clone();
                        let selected = sources.iter().any(|s| s.eq_ignore_ascii_case(&name));
                        Button::new(SharedString::from(format!("src-{name}")))
                            .small()
                            .label(name.clone())
                            .selected(selected)
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    this.toggle_source(&name, window, cx);
                                    cx.notify();
                                });
                            })
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().child(Input::new(&self.contains)))
                    .children(REGION_PRESETS.iter().map(|(label, tokens)| {
                        let entity = entity.clone();
                        let tokens: Vec<String> = tokens.iter().map(|t| (*t).to_string()).collect();
                        Button::new(SharedString::from(format!("region-{label}")))
                            .small()
                            .label(*label)
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    let refs: Vec<&str> = tokens.iter().map(String::as_str).collect();
                                    this.append_contains(&refs, window, cx);
                                    cx.notify();
                                });
                            })
                    })),
            )
            .when(!contains.is_empty(), |this| {
                this.child(chip_row(
                    entity.clone(),
                    "contains",
                    ChipField::Contains,
                    &contains,
                ))
            })
            .child(Input::new(&self.excludes))
            .when(!excludes.is_empty(), |this| {
                this.child(chip_row(
                    entity.clone(),
                    "excludes",
                    ChipField::Excludes,
                    &excludes,
                ))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(format!("预览 · {} 个节点 · {}", members.len(), draft.policy_label())),
            )
            .child(
                v_flex()
                    .id("group-preview")
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .rounded(radius)
                    .border_1()
                    .border_color(border)
                    .bg(group_box)
                    .when(members.is_empty() && self.blocked.is_empty(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(if parent.catalog.nodes.is_empty() {
                                    "目录是空的。先到订阅页 Apply。".to_string()
                                } else {
                                    "没有成员。放宽条件，或钉住节点。".to_string()
                                }),
                        )
                    })
                    .children(members.iter().take(PREVIEW_LIMIT).map(|name| {
                        render_member_row(
                            entity.clone(),
                            &theme,
                            name,
                            self.include.iter().any(|n| n == name),
                            false,
                        )
                    }))
                    .when(extra > 0, |this| {
                        this.child(
                            div()
                                .px_3()
                                .py_2()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(format!("其余 {extra} 个未列出。收紧条件或排除后再看。")),
                        )
                    })
                    .children(
                        self.blocked
                            .iter()
                            .filter(|name| !members.iter().any(|m| m == *name))
                            .map(|name| render_member_row(entity.clone(), &theme, name, false, true)),
                    ),
            )
    }
}

fn default_via(strategy: &Strategy) -> String {
    strategy
        .groups
        .iter()
        .find(|g| g.name == "PROXY")
        .or_else(|| strategy.groups.first())
        .map(|g| g.name.clone())
        .unwrap_or_else(|| "DIRECT".into())
}

fn via_label(via: &str) -> String {
    match via.trim().to_ascii_lowercase().as_str() {
        "direct" => "直连".into(),
        "reject" => "拒绝".into(),
        _ => via.trim().to_string(),
    }
}

fn via_choices(strategy: &Strategy, extra: Option<&str>) -> Vec<ViaChoice> {
    let mut out = vec![
        ViaChoice {
            value: "DIRECT".into(),
            label: "直连".into(),
        },
        ViaChoice {
            value: "REJECT".into(),
            label: "拒绝".into(),
        },
    ];
    for group in &strategy.groups {
        if out.iter().any(|c| c.value.eq_ignore_ascii_case(&group.name)) {
            continue;
        }
        out.push(ViaChoice {
            value: group.name.clone(),
            label: group.name.clone(),
        });
    }
    if let Some(via) = extra.map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|c| c.value.eq_ignore_ascii_case(via)) {
            out.push(ViaChoice {
                value: via.to_string(),
                label: via_label(via),
            });
        }
    }
    out
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
    group_modal_open: bool,
    group_edit_id: Option<String>,
    rule_match: Entity<InputState>,
    rule_via: String,
    rule_query: Entity<InputState>,
    filter_input: Entity<InputState>,
    port_input: Entity<InputState>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let strategy = match Strategy::load() {
            Ok(strategy) => strategy,
            Err(err) => {
                log::error("ui", format!("load strategy failed: {err:#}"));
                Strategy::default()
            }
        };
        let catalog = Catalog::load().unwrap_or_default();
        log::set_developer(strategy.developer_mode);
        log::info(
            "ui",
            format!(
                "window ready nodes={} groups={} rules={}",
                catalog.nodes.len(),
                strategy.groups.len(),
                strategy.rules.len()
            ),
        );
        let strategy_path = myproxy::paths::strategy_path().ok();
        let catalog_path = myproxy::paths::catalog_path().ok();
        cx.spawn(async move |this, cx| {
            let mut strategy_stamp = strategy_path.as_deref().and_then(file_stamp);
            let mut catalog_stamp = catalog_path.as_deref().and_then(file_stamp);
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(1500))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        let started = Instant::now();
                        let mut dirty = false;
                        let connected = this.supervisor.is_running();
                        if connected != this.connected {
                            this.connected = connected;
                            dirty = true;
                        }
                        let editing = this.group_modal_open || this.rule_edit_id.is_some();
                        if !editing {
                            if let Some(path) = strategy_path.as_deref() {
                                let stamp = file_stamp(path);
                                if stamp != strategy_stamp {
                                    strategy_stamp = stamp;
                                    if let Ok(strategy) = Strategy::load() {
                                        log::set_developer(strategy.developer_mode);
                                        this.strategy = strategy;
                                        dirty = true;
                                        log::debug("ui", "reload strategy.json");
                                    }
                                }
                            }
                            if let Some(path) = catalog_path.as_deref() {
                                let stamp = file_stamp(path);
                                if stamp != catalog_stamp {
                                    catalog_stamp = stamp;
                                    if let Ok(catalog) = Catalog::load() {
                                        log::debug(
                                            "ui",
                                            format!(
                                                "reload catalog.json nodes={}",
                                                catalog.nodes.len()
                                            ),
                                        );
                                        this.catalog = catalog;
                                        dirty = true;
                                    }
                                }
                            }
                        }
                        if this.page == Page::Settings && log::developer() {
                            dirty = true;
                        }
                        if dirty {
                            cx.notify();
                        }
                        let ms = started.elapsed().as_millis();
                        if ms >= 8 {
                            log::trace("ui", format!("poll {ms}ms dirty={dirty}"));
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let supervisor = Arc::new(Supervisor::default());
        let connected = supervisor.is_running();
        Self {
            page: initial_page(),
            status: "策略已加载。用 CLI 或本页编辑，然后点「更新配置」或「连接」。".into(),
            connected,
            supervisor,
            url_input: cx.new(|cx| {
                InputState::new(window, cx).placeholder("https://…/clash.yaml")
            }),
            name_input: cx.new(|cx| InputState::new(window, cx).placeholder("订阅名")),
            group_modal_open: false,
            group_edit_id: None,
            rule_draft_kind: RuleDraftKind::Suffix,
            rule_edit_id: None,
            rule_match: cx.new(|cx| {
                InputState::new(window, cx).placeholder(RuleDraftKind::Suffix.placeholder())
            }),
            rule_via: default_via(&strategy),
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
                log::error("ui", format!("save strategy failed: {err:#}"));
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
                    Err(err) => {
                        log::error("ui", format!("apply failed: {err:#}"));
                        this.status = format!("Apply 失败: {err:#}");
                    }
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
                        Err(err) => {
                            log::error("ui", format!("disconnect failed: {err:#}"));
                            this.status = format!("{err:#}");
                        }
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
                        Err(err) => {
                            log::error("ui", format!("connect failed: {err:#}"));
                            this.status = format!("{err:#}");
                        }
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
        self.rule_via = rule.via.clone();
        self.rule_match.update(cx, |input, cx| {
            input.set_value(match_value, window, cx);
        });
        self.status = format!("正在编辑 {} → {}", rule.match_label(), via_label(&rule.via));
    }

    fn cancel_edit_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rule_edit_id = None;
        self.rule_match.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.set_placeholder(self.rule_draft_kind.placeholder(), window, cx);
        });
        self.rule_via = default_via(&self.strategy);
        self.status = "已取消编辑。".into();
    }

    fn commit_rule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let match_value = self.rule_match.read(cx).value().trim().to_string();
        let via = self.rule_via.trim().to_string();
        if match_value.is_empty() {
            self.status = format!("填写{}。", self.rule_draft_kind.placeholder());
            return;
        }
        if via.is_empty() {
            self.status = "选一个走向：直连、拒绝，或一个节点组。".into();
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

    fn open_group_dialog(&mut self, id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        window.close_all_dialogs(cx);
        let existing = id.and_then(|id| self.strategy.groups.iter().find(|g| g.id == id).cloned());
        if id.is_some() && existing.is_none() {
            self.status = "找不到这个节点组。".into();
            return;
        }
        self.group_modal_open = true;
        self.group_edit_id = existing.as_ref().map(|g| g.id.clone());
        log::debug(
            "ui",
            format!(
                "open group dialog {}",
                existing
                    .as_ref()
                    .map(|g| g.name.as_str())
                    .unwrap_or("(new)")
            ),
        );
        let parent = cx.entity();
        let editor = cx.new(|cx| GroupEditor::new(parent.clone(), existing, window, cx));
        let editing = id.is_some();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(if editing {
                    "编辑节点组"
                } else {
                    "添加节点组"
                })
                .width(px(720.))
                .overlay_closable(true)
                .button_props(
                    DialogButtonProps::default()
                        .ok_text(if editing { "保存" } else { "添加" })
                        .cancel_text("取消")
                        .show_cancel(true)
                        .on_ok({
                            let editor = editor.clone();
                            move |_, window, cx| editor.update(cx, |ed, cx| ed.commit(window, cx))
                        })
                        .on_cancel({
                            let parent = parent.clone();
                            move |_, _, cx| {
                                parent.update(cx, |this, cx| {
                                    this.group_modal_open = false;
                                    this.group_edit_id = None;
                                    cx.notify();
                                });
                                true
                            }
                        }),
                )
                .on_close({
                    let parent = parent.clone();
                    move |_, _, cx| {
                        parent.update(cx, |this, cx| {
                            this.group_modal_open = false;
                            this.group_edit_id = None;
                            cx.notify();
                        });
                    }
                })
                .child(editor.clone())
        });
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .size_full()
            .child(
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
                    ),
            )
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
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
        let accent = theme.accent;
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "节点组",
                "点卡片打开编辑窗。来源 ∩ 名称含（或，支持 * ?）∪ 钉住 − 排除。名称不含只打自动命中。",
            ))
            .child(
                h_flex().child({
                    let entity = entity.clone();
                    Button::new("add-group")
                        .primary()
                        .label("添加节点组")
                        .on_click(move |_, window, app| {
                            entity.update(app, |this, cx| {
                                this.open_group_dialog(None, window, cx);
                                cx.notify();
                            });
                        })
                }),
            )
            .when(self.strategy.groups.is_empty(), |this| {
                this.child(empty_hint(theme, "还没有节点组。默认会有 PROXY 组。"))
            })
            .children(self.strategy.groups.iter().map(|group| {
                let count = catalog::count_group_members(group, &self.catalog);
                let selected = self.group_edit_id.as_deref() == Some(group.id.as_str());
                render_group_card(entity.clone(), theme, group, count, selected, accent)
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
                            .child({
                                let entity = entity.clone();
                                let current = self.rule_via.clone();
                                let choices = via_choices(&self.strategy, Some(&current));
                                Button::new("rule-via")
                                    .small()
                                    .label(via_label(&current))
                                    .icon(IconName::ChevronDown)
                                    .min_w(px(140.))
                                    .dropdown_menu(move |menu, _, _| {
                                        let mut menu = menu.scrollable(true).min_w(px(168.));
                                        for (i, choice) in choices.iter().enumerate() {
                                            if i == 2 {
                                                menu = menu.separator();
                                            }
                                            let entity = entity.clone();
                                            let value = choice.value.clone();
                                            let checked = current.eq_ignore_ascii_case(&choice.value);
                                            menu = menu.item(
                                                PopupMenuItem::new(choice.label.clone())
                                                    .checked(checked)
                                                    .on_click(move |_, _, app| {
                                                        entity.update(app, |this, cx| {
                                                            this.rule_via = value.clone();
                                                            cx.notify();
                                                        });
                                                    }),
                                            );
                                        }
                                        menu
                                    })
                            })
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
                            .child("走向：直连、拒绝，或任意节点组。"),
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
                                            via_choices(&self.strategy, Some(&rule.via)),
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
            .child(self.developer_panel(cx, theme))
    }

    fn developer_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let on = self.strategy.developer_mode || log::env_forced();
        let log_path = log::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(无法创建日志文件)".into());
        panel(
            theme,
            "开发者",
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("error/warn/info 始终写入日志文件；debug/trace 仅开发者模式。不记录订阅 URL。MYPROXY_DEV=1 也会打开。"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child({
                            let entity = entity.clone();
                            let mut toggle = Button::new("dev-mode").small();
                            toggle = if on {
                                toggle.danger().label("关闭开发者模式")
                            } else {
                                toggle.primary().label("开启开发者模式")
                            };
                            toggle.on_click(move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    this.strategy.developer_mode = !this.strategy.developer_mode;
                                    log::set_developer(this.strategy.developer_mode);
                                    this.persist();
                                    cx.notify();
                                });
                            })
                        })
                        .when(on, |this| {
                            this.child({
                                Button::new("reveal-log")
                                    .small()
                                    .label("在 Finder 中显示")
                                    .on_click(move |_, _, _| {
                                        if let Some(path) = log::path() {
                                            let _ = std::process::Command::new("open")
                                                .arg("-R")
                                                .arg(path)
                                                .spawn();
                                        }
                                    })
                            })
                        }),
                )
                .when(on, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .font_family(theme.mono_font_family.clone())
                            .text_color(theme.muted_foreground)
                            .child(log_path),
                    )
                    .child(
                        v_flex()
                            .id("dev-log")
                            .max_h(px(240.))
                            .overflow_y_scroll()
                            .p_3()
                            .gap_1()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.group_box)
                            .children(log::recent(80).into_iter().map(|line| {
                                div()
                                    .text_xs()
                                    .font_family(theme.mono_font_family.clone())
                                    .child(line)
                            })),
                    )
                }),
        )
    }
}

#[derive(Clone, Copy)]
enum ChipField {
    Contains,
    Excludes,
}

fn chip_row(
    entity: Entity<GroupEditor>,
    prefix: &str,
    field: ChipField,
    tokens: &[String],
) -> impl IntoElement {
    h_flex().gap_1().flex_wrap().children(tokens.iter().map(|token| {
        let entity = entity.clone();
        let token = token.clone();
        let id = SharedString::from(format!("{prefix}-{token}"));
        Button::new(id)
            .small()
            .label(format!("{token} ×"))
            .on_click(move |_, window, app| {
                entity.update(app, |this, cx| {
                    match field {
                        ChipField::Contains => {
                            GroupEditor::remove_token(&this.contains, &token, window, cx);
                        }
                        ChipField::Excludes => {
                            GroupEditor::remove_token(&this.excludes, &token, window, cx);
                        }
                    }
                    cx.notify();
                });
            })
    }))
}

fn render_member_row(
    entity: Entity<GroupEditor>,
    theme: &Theme,
    name: &str,
    pinned: bool,
    blocked: bool,
) -> impl IntoElement {
    let name_owned = name.to_string();
    let muted_fg = theme.muted_foreground;
    let accent = theme.accent;
    h_flex()
        .id(SharedString::from(format!("member-{name}")))
        .w_full()
        .px_3()
        .py_1()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_xs()
                .when(blocked, |this| this.text_color(muted_fg))
                .child(name.to_string()),
        )
        .when(pinned, |this| this.child(pill(theme, "钉住", accent)))
        .when(blocked, |this| this.child(pill(theme, "排除", theme.warning)))
        .when(pinned, |this| {
            let entity = entity.clone();
            let name = name_owned.clone();
            this.child(
                Button::new(SharedString::from(format!("unpin-{name}")))
                    .small()
                    .label("取消钉住")
                    .on_click(move |_, _, app| {
                        entity.update(app, |this, cx| {
                            this.unpin_member(&name);
                            cx.notify();
                        });
                    }),
            )
        })
        .when(blocked && !pinned, |this| {
            let entity = entity.clone();
            let name = name_owned.clone();
            this.child(
                Button::new(SharedString::from(format!("unblock-{name}")))
                    .small()
                    .label("取消排除")
                    .on_click(move |_, _, app| {
                        entity.update(app, |this, cx| {
                            this.unblock_member(&name);
                            cx.notify();
                        });
                    }),
            )
        })
        .when(!pinned && !blocked, |this| {
            let pin_entity = entity.clone();
            let pin_name = name_owned.clone();
            let block_entity = entity.clone();
            let block_name = name_owned.clone();
            this.child(
                Button::new(SharedString::from(format!("pin-{pin_name}")))
                    .small()
                    .label("钉住")
                    .on_click(move |_, _, app| {
                        pin_entity.update(app, |this, cx| {
                            this.pin_member(&pin_name);
                            cx.notify();
                        });
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("block-{block_name}")))
                    .small()
                    .label("排除")
                    .on_click(move |_, _, app| {
                        block_entity.update(app, |this, cx| {
                            this.block_member(&block_name);
                            cx.notify();
                        });
                    }),
            )
        })
}

fn render_group_card(
    entity: Entity<AppView>,
    theme: &Theme,
    group: &Group,
    count: usize,
    selected: bool,
    accent: Hsla,
) -> impl IntoElement {
    let id = group.id.clone();
    let del_id = group.id.clone();
    let muted = theme.muted;
    let muted_fg = theme.muted_foreground;
    v_flex()
        .id(SharedString::from(format!("group-card-{id}")))
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .cursor_pointer()
        .when(selected, |this| this.bg(accent.opacity(0.14)))
        .hover(move |style| style.bg(muted))
        .on_click({
            let entity = entity.clone();
            let id = id.clone();
            move |_, window, app| {
                entity.update(app, |this, cx| {
                    this.open_group_dialog(Some(&id), window, cx);
                    cx.notify();
                });
            }
        })
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(format!("{}  ·  {}  ·  {} 个节点", group.name, group.kind, count)),
                )
                .child({
                    let entity = entity.clone();
                    Button::new(SharedString::from(format!("del-group-{del_id}")))
                        .small()
                        .danger()
                        .label("删除")
                        .on_click(move |_, window, app| {
                            app.stop_propagation();
                            let close_modal = entity
                                .read(app)
                                .group_edit_id
                                .as_deref()
                                == Some(del_id.as_str());
                            entity.update(app, |this, cx| {
                                if close_modal {
                                    this.group_modal_open = false;
                                    this.group_edit_id = None;
                                }
                                this.strategy.remove_group(&del_id);
                                this.persist();
                                cx.notify();
                            });
                            if close_modal {
                                window.close_dialog(app);
                            }
                        })
                }),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted_fg)
                .child(group.policy_label()),
        )
}

fn file_stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
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
        .child(div().w(px(168.)).flex_shrink_0().child(via))
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
    via_choices: Vec<ViaChoice>,
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
        pill(theme, &via_label(&via), accent),
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
                let choices = via_choices.clone();
                move |menu, _, _| {
                    let mut menu = menu.scrollable(true).min_w(px(168.));
                    for (i, choice) in choices.iter().enumerate() {
                        if i == 2 {
                            menu = menu.separator();
                        }
                        let entity = entity.clone();
                        let id = id.clone();
                        let value = choice.value.clone();
                        let checked = via_current.eq_ignore_ascii_case(&choice.value);
                        menu = menu.item(
                            PopupMenuItem::new(choice.label.clone())
                                .checked(checked)
                                .on_click(move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.set_selected_rule_via(&id, &value);
                                        cx.notify();
                                    });
                                }),
                        );
                    }
                    menu
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
