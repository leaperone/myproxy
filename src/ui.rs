use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::button::{
    Button, ButtonGroup, ButtonVariants as _, Toggle, ToggleGroup, ToggleVariants as _,
};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogButtonProps, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
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
use myproxy::controller::{self, TrafficSnapshot};
use myproxy::log;
use myproxy::strategy::{join_list, parse_list, Group, Matcher, RuleSet, Strategy};
use myproxy::supervisor::Supervisor;

use crate::appearance::Appearance;

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleDraftKind {
    App,
    Exact,
    Suffix,
    Keyword,
    Cidr,
}

impl RuleDraftKind {
    const ALL: [Self; 5] = [
        Self::App,
        Self::Exact,
        Self::Suffix,
        Self::Keyword,
        Self::Cidr,
    ];

    fn from_matcher(matcher: &Matcher) -> Self {
        match matcher.kind.as_str() {
            "app" => Self::App,
            "keyword" => Self::Keyword,
            "domain" => Self::Exact,
            "cidr" => Self::Cidr,
            _ => Self::Suffix,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::App => "进程",
            Self::Exact => "域名",
            Self::Suffix => "后缀",
            Self::Keyword => "关键字",
            Self::Cidr => "网段",
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::App => "进程名，例如 Arc",
            Self::Exact => "apple.com",
            Self::Suffix => "apple.com，匹配其子域",
            Self::Keyword => "关键字，例如 google",
            Self::Cidr => "149.154.160.0/20",
        }
    }

    fn into_matcher(self, match_value: String) -> Matcher {
        match self {
            Self::App => Matcher::app(match_value),
            Self::Exact => Matcher::domain(match_value),
            Self::Suffix => Matcher::suffix(match_value),
            Self::Keyword => Matcher::keyword(match_value),
            Self::Cidr => Matcher::cidr(match_value),
        }
    }
}

#[derive(Clone)]
struct ViaChoice {
    value: String,
    label: String,
    section: u8,
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

struct RuleSetEditor {
    parent: Entity<AppView>,
    edit_id: Option<String>,
    notice: String,
    name: Entity<InputState>,
    via: String,
    matchers: Vec<Matcher>,
    draft_kind: RuleDraftKind,
    match_input: Entity<InputState>,
}

impl RuleSetEditor {
    fn new(
        parent: Entity<AppView>,
        existing: Option<RuleSet>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let via = existing
            .as_ref()
            .map(|s| s.via.clone())
            .unwrap_or_else(|| default_via(&parent.read(cx).strategy));
        let name = existing
            .as_ref()
            .map(|s| s.name.clone())
            .unwrap_or_default();
        let matchers = existing
            .as_ref()
            .map(|s| s.matchers.clone())
            .unwrap_or_default();
        let draft_kind = matchers
            .last()
            .map(RuleDraftKind::from_matcher)
            .unwrap_or(RuleDraftKind::Suffix);
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("项目名，例如 Cursor")
                .default_value(name)
        });
        let match_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(draft_kind.placeholder())
        });
        Self {
            parent,
            edit_id: existing.as_ref().map(|s| s.id.clone()),
            notice: String::new(),
            name,
            via,
            matchers,
            draft_kind,
            match_input,
        }
    }

    fn draft(&self, cx: &App) -> RuleSet {
        RuleSet {
            id: self.edit_id.clone().unwrap_or_default(),
            name: self.name.read(cx).value().trim().to_string(),
            via: self.via.trim().to_string(),
            matchers: self.matchers.clone(),
        }
    }

    fn add_matchers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let raw = self.match_input.read(cx).value();
        let parts = parse_list(&raw);
        if parts.is_empty() {
            self.notice = format!("填写{}。", self.draft_kind.placeholder());
            cx.notify();
            return;
        }
        for part in parts {
            let matcher = self.draft_kind.into_matcher(part);
            if matcher.value.is_empty() {
                continue;
            }
            if !self.matchers.iter().any(|m| m.same_as(&matcher)) {
                self.matchers.push(matcher);
            }
        }
        self.match_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        self.notice.clear();
        cx.notify();
    }

    fn commit(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut next = self.draft(cx);
        if next.name.is_empty() {
            self.notice = "需要项目名。".into();
            cx.notify();
            return false;
        }
        if next.matchers.is_empty() {
            self.notice = "至少加一条匹配。".into();
            cx.notify();
            return false;
        }
        if next.via.is_empty() {
            self.notice = "选走向：直连、拒绝、节点组，或一个节点。".into();
            cx.notify();
            return false;
        }
        let edit_id = self.edit_id.clone();
        let result = self.parent.update(cx, |parent, cx| {
            if let Some(id) = &edit_id {
                if !parent.strategy.update_rule_set(id, next) {
                    return Err("保存失败：规则已不存在。".to_string());
                }
            } else if let Some(index) = parent
                .strategy
                .rule_sets
                .iter()
                .position(|s| s.name.eq_ignore_ascii_case(&next.name))
            {
                let mut merged = parent.strategy.rule_sets[index].clone();
                for matcher in next.matchers {
                    if !merged.matchers.iter().any(|m| m.same_as(&matcher)) {
                        merged.matchers.push(matcher);
                    }
                }
                merged.via = next.via;
                let id = merged.id.clone();
                parent.strategy.update_rule_set(&id, merged);
            } else {
                next.id = uuid::Uuid::new_v4().to_string();
                parent.strategy.add_rule_set(next);
            }
            if parent.persist() {
                parent.rule_modal_open = false;
                parent.rule_edit_id = None;
                parent.status = if edit_id.is_some() {
                    "已保存规则。".into()
                } else {
                    "已添加规则。".into()
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
}

impl Render for RuleSetEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let parent = self.parent.read(cx);
        let theme = cx.theme().clone();
        let muted_fg = theme.muted_foreground;
        let via = self.via.clone();
        let choices = via_choices(&parent.strategy, &parent.catalog, Some(&via));

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
                    .child(div().flex_1().child(Input::new(&self.name)))
                    .child({
                        let entity = entity.clone();
                        Button::new("rule-via")
                            .small()
                            .label(via_label(&via))
                            .icon(IconName::ChevronDown)
                            .min_w(px(160.))
                            .dropdown_menu({
                                let entity = entity.clone();
                                move |menu, _, _| {
                                    let entity = entity.clone();
                                    via_menu(menu, &choices, &via, move |app, value| {
                                        entity.update(app, |this, cx| {
                                            this.via = value;
                                            cx.notify();
                                        });
                                    })
                                }
                            })
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
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
                                .selected(self.draft_kind == kind),
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
                                this.draft_kind = kind;
                                this.match_input.update(cx, |input, cx| {
                                    input.set_placeholder(kind.placeholder(), window, cx);
                                });
                                cx.notify();
                            });
                        })
                    })
                    .child(div().flex_1().child(Input::new(&self.match_input)))
                    .child({
                        let entity = entity.clone();
                        Button::new("add-matcher")
                            .small()
                            .label("加入")
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    this.add_matchers(window, cx);
                                });
                            })
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("逗号分隔可一次加入多条。走向可是节点组，或目录里的某个节点。"),
            )
            .when(self.matchers.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted_fg)
                        .child("还没有匹配项。"),
                )
            })
            .child(
                h_flex().gap_1().flex_wrap().children(self.matchers.iter().enumerate().map(
                    |(index, matcher)| {
                        let entity = entity.clone();
                        let id = SharedString::from(format!(
                            "matcher-{}-{}-{index}",
                            matcher.kind, matcher.value
                        ));
                        Button::new(id)
                            .small()
                            .label(format!(
                                "{} {} ×",
                                matcher.kind_label(),
                                matcher.display_value()
                            ))
                            .on_click(move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    if index < this.matchers.len() {
                                        this.matchers.remove(index);
                                    }
                                    cx.notify();
                                });
                            })
                    },
                )),
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
    let via = via.trim();
    match via.to_ascii_lowercase().as_str() {
        "direct" => "直连".into(),
        "reject" => "拒绝".into(),
        _ => via
            .strip_prefix("node:")
            .unwrap_or(via)
            .to_string(),
    }
}

fn via_choices(strategy: &Strategy, catalog: &Catalog, extra: Option<&str>) -> Vec<ViaChoice> {
    let mut out = vec![
        ViaChoice {
            value: "DIRECT".into(),
            label: "直连".into(),
            section: 0,
        },
        ViaChoice {
            value: "REJECT".into(),
            label: "拒绝".into(),
            section: 0,
        },
    ];
    for group in &strategy.groups {
        if out.iter().any(|c| c.value.eq_ignore_ascii_case(&group.name)) {
            continue;
        }
        out.push(ViaChoice {
            value: group.name.clone(),
            label: group.name.clone(),
            section: 1,
        });
    }
    for node in &catalog.nodes {
        let value = if strategy.groups.iter().any(|g| g.name == node.name) {
            format!("node:{}", node.name)
        } else {
            node.name.clone()
        };
        if out.iter().any(|c| c.value.eq_ignore_ascii_case(&value)) {
            continue;
        }
        out.push(ViaChoice {
            value,
            label: node.name.clone(),
            section: 2,
        });
    }
    if let Some(via) = extra.map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|c| c.value.eq_ignore_ascii_case(via)) {
            out.push(ViaChoice {
                value: via.to_string(),
                label: via_label(via),
                section: 2,
            });
        }
    }
    out
}

fn via_menu(
    mut menu: PopupMenu,
    choices: &[ViaChoice],
    current: &str,
    on_pick: impl Fn(&mut App, String) + Clone + 'static,
) -> PopupMenu {
    let mut last_section = 0u8;
    menu = menu.scrollable(true).min_w(px(200.));
    for choice in choices {
        if choice.section != last_section {
            menu = menu.separator();
            last_section = choice.section;
        }
        let value = choice.value.clone();
        let checked = current.eq_ignore_ascii_case(&choice.value);
        let on_pick = on_pick.clone();
        menu = menu.item(
            PopupMenuItem::new(choice.label.clone())
                .checked(checked)
                .on_click(move |_, _, app| on_pick(app, value.clone())),
        );
    }
    menu
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Overview,
    Connections,
    Subscriptions,
    Groups,
    Rules,
    Settings,
}

fn initial_page() -> Page {
    match std::env::var("MYPROXY_PAGE").unwrap_or_default().as_str() {
        "connections" | "traffic" | "conn" => Page::Connections,
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
    group_modal_open: bool,
    group_edit_id: Option<String>,
    rule_modal_open: bool,
    rule_edit_id: Option<String>,
    rule_query: Entity<InputState>,
    filter_input: Entity<InputState>,
    port_input: Entity<InputState>,
    appearance: Appearance,
    _appearance_observer: Subscription,
    traffic: TrafficSnapshot,
    traffic_up: u64,
    traffic_down: u64,
    traffic_has_rate: bool,
    traffic_error: Option<String>,
    traffic_prev: Option<(Instant, u64, u64)>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = Appearance::load();
        appearance.apply(Some(window), cx);
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
                "window ready nodes={} groups={} rule_sets={}",
                catalog.nodes.len(),
                strategy.groups.len(),
                strategy.rule_sets.len()
            ),
        );
        let strategy_path = myproxy::paths::strategy_path().ok();
        let catalog_path = myproxy::paths::catalog_path().ok();
        cx.spawn(async move |this, cx| {
            let mut strategy_stamp = strategy_path.as_deref().and_then(file_stamp);
            let mut catalog_stamp = catalog_path.as_deref().and_then(file_stamp);
            loop {
                let wait_ms = this
                    .update(cx, |this, _| {
                        if this.page == Page::Connections {
                            1000
                        } else {
                            1500
                        }
                    })
                    .unwrap_or(1500);
                cx.background_executor()
                    .timer(Duration::from_millis(wait_ms))
                    .await;

                let fetch_port = this
                    .update(cx, |this, cx| {
                        if !this.connected {
                            if this.clear_traffic() {
                                cx.notify();
                            }
                            None
                        } else if this.page == Page::Connections {
                            Some(this.strategy.mixed_port)
                        } else {
                            None
                        }
                    })
                    .ok()
                    .flatten();
                if let Some(port) = fetch_port {
                    let result = cx
                        .background_executor()
                        .spawn(async move {
                            controller::fetch(port).map_err(|err| err.to_string())
                        })
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            this.apply_traffic(result);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }

                if this
                    .update(cx, |this, cx| {
                        let started = Instant::now();
                        let mut dirty = false;
                        let connected = this.supervisor.is_running();
                        if connected != this.connected {
                            this.connected = connected;
                            if !connected {
                                this.clear_traffic();
                            }
                            dirty = true;
                        }
                        let editing = this.group_modal_open || this.rule_modal_open;
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
        let entity = cx.entity();
        let appearance_observer = window.observe_window_appearance(move |window, cx| {
            entity.update(cx, |this, cx| {
                if this.appearance == Appearance::System {
                    Theme::sync_system_appearance(Some(window), cx);
                    cx.notify();
                }
            });
        });
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
            rule_modal_open: false,
            rule_edit_id: None,
            rule_query: cx.new(|cx| InputState::new(window, cx).placeholder("筛选规则…")),
            filter_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(strategy.exclude_filter.clone())
            }),
            port_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(strategy.mixed_port.to_string())
            }),
            strategy,
            catalog,
            appearance,
            _appearance_observer: appearance_observer,
            traffic: TrafficSnapshot::default(),
            traffic_up: 0,
            traffic_down: 0,
            traffic_has_rate: false,
            traffic_error: None,
            traffic_prev: None,
        }
    }

    fn set_appearance(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&Vec<bool>, &mut Window, &mut App) + 'static {
        let entity = cx.entity();
        move |checks, window, app| {
            entity.update(app, |this, cx| {
                let current = [
                    this.appearance == Appearance::Light,
                    this.appearance == Appearance::Dark,
                    this.appearance == Appearance::System,
                ];
                let appearance = if checks.first() != Some(&current[0]) {
                    Appearance::Light
                } else if checks.get(1) != Some(&current[1]) {
                    Appearance::Dark
                } else if checks.get(2) != Some(&current[2]) {
                    Appearance::System
                } else {
                    return;
                };
                if this.appearance == appearance {
                    return;
                }
                this.appearance = appearance;
                this.appearance.save();
                this.appearance.apply(Some(window), cx);
                cx.notify();
            });
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

    fn clear_traffic(&mut self) -> bool {
        let dirty = !self.traffic.connections.is_empty()
            || self.traffic_has_rate
            || self.traffic_error.is_some()
            || self.traffic.upload_total != 0
            || self.traffic.download_total != 0;
        self.traffic = TrafficSnapshot::default();
        self.traffic_up = 0;
        self.traffic_down = 0;
        self.traffic_has_rate = false;
        self.traffic_error = None;
        self.traffic_prev = None;
        dirty
    }

    fn apply_traffic(&mut self, result: Result<TrafficSnapshot, String>) {
        match result {
            Ok(snap) => {
                let now = Instant::now();
                if let Some((prev_at, prev_up, prev_down)) = self.traffic_prev {
                    let dt = now.duration_since(prev_at).as_secs_f64();
                    if dt >= 0.2 {
                        self.traffic_up =
                            ((snap.upload_total.saturating_sub(prev_up)) as f64 / dt) as u64;
                        self.traffic_down =
                            ((snap.download_total.saturating_sub(prev_down)) as f64 / dt) as u64;
                        self.traffic_has_rate = true;
                        self.traffic_prev = Some((now, snap.upload_total, snap.download_total));
                    }
                } else {
                    self.traffic_prev = Some((now, snap.upload_total, snap.download_total));
                }
                self.traffic = snap;
                self.traffic_error = None;
            }
            Err(err) => {
                log::debug("ui", format!("connections poll failed: {err}"));
                self.traffic_error = Some("暂时读不到核心连接。".into());
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
                            "已编译 {} 个节点，排除 {}。Mixed {}{}。",
                            this.catalog.nodes.len(),
                            this.catalog.excluded.len(),
                            this.strategy.mixed_port,
                            if this.strategy.tun { " + TUN" } else { "" }
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
                                "已连接 127.0.0.1:{} （HTTP + SOCKS5{}）",
                                this.strategy.mixed_port,
                                if this.strategy.tun { " + TUN" } else { "" }
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

    fn open_rule_dialog(&mut self, id: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        window.close_all_dialogs(cx);
        let existing = id.and_then(|id| self.strategy.rule_sets.iter().find(|s| s.id == id).cloned());
        if id.is_some() && existing.is_none() {
            self.status = "找不到这条规则。".into();
            return;
        }
        self.rule_modal_open = true;
        self.rule_edit_id = existing.as_ref().map(|s| s.id.clone());
        log::debug(
            "ui",
            format!(
                "open rule dialog {}",
                existing
                    .as_ref()
                    .map(|s| s.name.as_str())
                    .unwrap_or("(new)")
            ),
        );
        let parent = cx.entity();
        let editor = cx.new(|cx| RuleSetEditor::new(parent.clone(), existing, window, cx));
        let editing = id.is_some();
        window.open_dialog(cx, move |dialog, window, _| {
            let ok_label = if editing { "保存" } else { "添加" };
            dialog
                .title(if editing {
                    "编辑规则"
                } else {
                    "添加规则"
                })
                .width(px(640.))
                .max_h(window.viewport_size().height - px(96.))
                .overlay_closable(true)
                .button_props(
                    DialogButtonProps::default()
                        .on_ok({
                            let editor = editor.clone();
                            move |_, window, cx| editor.update(cx, |ed, cx| ed.commit(window, cx))
                        })
                        .on_cancel({
                            let parent = parent.clone();
                            move |_, _, cx| {
                                parent.update(cx, |this, cx| {
                                    this.rule_modal_open = false;
                                    this.rule_edit_id = None;
                                    cx.notify();
                                });
                                true
                            }
                        }),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("rule-dialog-cancel").label("取消").on_click(
                                |_, window, cx| window.dispatch_action(Box::new(Cancel), cx),
                            ),
                        )
                        .child(
                            Button::new("rule-dialog-ok")
                                .primary()
                                .label(ok_label)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(Confirm { secondary: false }),
                                        cx,
                                    )
                                }),
                        ),
                )
                .on_close({
                    let parent = parent.clone();
                    move |_, _, cx| {
                        parent.update(cx, |this, cx| {
                            this.rule_modal_open = false;
                            this.rule_edit_id = None;
                            cx.notify();
                        });
                    }
                })
                .child(editor.clone())
        });
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
        window.open_dialog(cx, move |dialog, window, _| {
            let ok_label = if editing { "保存" } else { "添加" };
            dialog
                .title(if editing {
                    "编辑节点组"
                } else {
                    "添加节点组"
                })
                .width(px(720.))
                .max_h(window.viewport_size().height - px(96.))
                .overlay_closable(true)
                .button_props(
                    DialogButtonProps::default()
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
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("group-dialog-cancel").label("取消").on_click(
                                |_, window, cx| window.dispatch_action(Box::new(Cancel), cx),
                            ),
                        )
                        .child(
                            Button::new("group-dialog-ok")
                                .primary()
                                .label(ok_label)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(Confirm { secondary: false }),
                                        cx,
                                    )
                                }),
                        ),
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
                self.rule_modal_open = false;
                self.rule_edit_id = None;
                window.close_dialog(cx);
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
                        .child(self.nav_item(cx, Page::Connections, "连接", IconName::Network))
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
                Page::Connections => self.connections(cx, theme).into_any_element(),
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
                        Page::Rules | Page::Connections => unreachable!(),
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
                        &self.strategy.rule_sets.len().to_string(),
                    )),
            )
    }

    fn connections(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let connected = self.connected;
        let muted_fg = theme.muted_foreground;
        let up = if connected && self.traffic_has_rate {
            controller::format_rate(self.traffic_up)
        } else {
            "—".into()
        };
        let down = if connected && self.traffic_has_rate {
            controller::format_rate(self.traffic_down)
        } else {
            "—".into()
        };
        let count = if connected {
            self.traffic.connections.len().to_string()
        } else {
            "—".into()
        };
        let mixed = format!("127.0.0.1:{}", self.strategy.mixed_port);
        v_flex()
            .id("connections-page")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .gap_4()
            .child(page_title(
                theme,
                "连接",
                "当前经过 Mixed 端口的流量。应用规则只在流量进入该端口后才会命中。",
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(metric(
                        theme,
                        "状态",
                        if connected { "已连接" } else { "未连接" },
                    ))
                    .child(metric(theme, "Mixed 端口", &mixed))
                    .child(metric(theme, "上传", &up))
                    .child(metric(theme, "下载", &down))
                    .child(metric(theme, "连接数", &count)),
            )
            .when(
                self.traffic_error.is_some()
                    || (connected && !self.traffic.connections.is_empty()),
                |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(self.traffic_error.clone().unwrap_or_default()),
                            )
                            .when(connected && !self.traffic.connections.is_empty(), |this| {
                                this.child({
                                    let entity = entity.clone();
                                    Button::new("close-all-connections")
                                        .small()
                                        .danger()
                                        .label("关闭全部")
                                        .on_click(move |_, _, app| {
                                            entity.update(app, |this, cx| {
                                                let port = this.strategy.mixed_port;
                                                this.traffic.connections.clear();
                                                this.status = "已关闭全部连接。".into();
                                                cx.background_executor()
                                                    .spawn(async move {
                                                        if let Err(err) =
                                                            controller::close_all(port)
                                                        {
                                                            log::debug(
                                                                "ui",
                                                                format!("close all failed: {err:#}"),
                                                            );
                                                        }
                                                    })
                                                    .detach();
                                                cx.notify();
                                            });
                                        })
                                })
                            }),
                    )
                },
            )
            .when(!connected, |this| {
                this.child(empty_hint(
                    theme,
                    "核心未连接。点右上角「连接」后，这里会显示经过 Mixed 端口的连接。",
                ))
            })
            .when(
                connected && self.traffic.connections.is_empty() && self.traffic_error.is_none(),
                |this| {
                    this.child(empty_hint(
                        theme,
                        "暂时没有连接。只有进入 Mixed 端口的流量会出现在这里。",
                    ))
                },
            )
            .when(
                connected && self.traffic.connections.is_empty() && self.traffic_error.is_some(),
                |this| {
                    this.child(empty_hint(
                        theme,
                        "核心已连接，但还读不到连接列表。等 Mixed 端口起来后再看。",
                    ))
                },
            )
            .when(connected && !self.traffic.connections.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id("connection-list")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .gap_1()
                        .child(connection_header_row(theme))
                        .children(self.traffic.connections.iter().map(|conn| {
                            render_connection_row(entity.clone(), theme, conn)
                        })),
                )
            })
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
        let query = self.rule_query.read(cx).value().to_string();
        let total = self.strategy.rule_sets.len();
        let visible: Vec<(usize, RuleSet)> = self
            .strategy
            .rule_sets
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, set)| set.matches_query(&query))
            .collect();
        let visible_len = visible.len();
        let muted_fg = theme.muted_foreground;
        let accent = theme.accent;
        v_flex()
            .id("rules-page")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .gap_4()
            .child(page_title(
                theme,
                "规则",
                "一个项目收一组进程和域名，整组走同一个节点组或节点。自上而下第一条命中。",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child({
                        let entity = entity.clone();
                        Button::new("add-rule")
                            .primary()
                            .label("添加规则")
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    this.open_rule_dialog(None, window, cx);
                                    cx.notify();
                                });
                            })
                    })
                    .child(div().flex_1().child(Input::new(&self.rule_query)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(if query.trim().is_empty() {
                                format!("{total} 项")
                            } else {
                                format!("{visible_len} / {total} 项")
                            }),
                    ),
            )
            .when(self.strategy.rule_sets.is_empty(), |this| {
                this.child(empty_hint(
                    theme,
                    "还没有规则。添加一个项目，把 Cursor、GitHub 这类收进去，再选走向。未匹配的流量走 PROXY。",
                ))
            })
            .when(
                !self.strategy.rule_sets.is_empty() && visible.is_empty(),
                |this| this.child(empty_hint(theme, "没有匹配筛选的规则。")),
            )
            .child(
                v_flex()
                    .id("rule-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .gap_3()
                    .children(visible.into_iter().map(|(index, set)| {
                        render_rule_set_card(
                            entity.clone(),
                            theme,
                            index,
                            total,
                            self.rule_edit_id.as_deref() == Some(set.id.as_str()),
                            &set,
                            via_choices(&self.strategy, &self.catalog, Some(&set.via)),
                            accent,
                        )
                    })),
            )
    }

    fn settings(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "设置",
                "Mixed 一口同时提供 HTTP 代理与 SOCKS5。TUN 可接管系统流量。排除器用正则。",
            ))
            .child(panel(
                theme,
                "外观",
                v_flex().gap_3().child(self.appearance_row(cx, theme)),
            ))
            .child(panel(
                theme,
                "入口",
                v_flex()
                    .gap_3()
                    .child(
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
                                            if let Ok(port) =
                                                this.port_input.read(cx).value().parse::<u16>()
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
                    )
                    .child(self.tun_row(cx, theme)),
            ))
            .child(self.updates_panel(cx, theme))
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

    fn tun_row(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let on = self.strategy.tun;
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_sm().child("系统接管 (TUN)"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("让 Telegram 等不走 Mixed 的流量进核心。首次连接会要一次管理员密码。改完需重新连接。"),
                    ),
            )
            .child({
                let mut toggle = Button::new("tun-toggle").small();
                toggle = if on {
                    toggle.danger().label("关闭")
                } else {
                    toggle.primary().label("开启")
                };
                toggle.on_click(move |_, _, app| {
                    entity.update(app, |this, cx| {
                        this.strategy.tun = !this.strategy.tun;
                        this.persist();
                        cx.notify();
                    });
                })
            })
    }

    fn appearance_row(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_sm().child("主题"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("浅色、深色，或跟随系统"),
                    ),
            )
            .child(
                ToggleGroup::new("appearance-mode")
                    .outline()
                    .segmented()
                    .small()
                    .child(
                        Toggle::new("appearance-light")
                            .label("浅色")
                            .checked(self.appearance == Appearance::Light),
                    )
                    .child(
                        Toggle::new("appearance-dark")
                            .label("深色")
                            .checked(self.appearance == Appearance::Dark),
                    )
                    .child(
                        Toggle::new("appearance-system")
                            .label("系统")
                            .checked(self.appearance == Appearance::System),
                    )
                    .on_click(self.set_appearance(cx)),
            )
    }

    fn updates_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let version = env!("CARGO_PKG_VERSION");
        let hint = if crate::sparkle::available() {
            "稳定渠道走 GitHub Releases 上的 Sparkle appcast。v0.0.1 是完整包，之后的版本才会生成增量 delta。"
        } else {
            "此开发构建未链接 Sparkle。打包脚本会带上更新器。"
        };
        panel(
            theme,
            "更新",
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_sm()
                        .child(format!("当前版本 {version}")),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(hint),
                )
                .child({
                    let entity = entity.clone();
                    Button::new("check-updates")
                        .label("检查更新")
                        .on_click(move |_, _, app| {
                            crate::sparkle::check();
                            entity.update(app, |this, cx| {
                                this.status = if crate::sparkle::available() {
                                    "已请求检查更新。".into()
                                } else {
                                    "此构建没有更新器。".into()
                                };
                                cx.notify();
                            });
                        })
                }),
        )
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

fn connection_header_row(theme: &Theme) -> impl IntoElement {
    let muted_fg = theme.muted_foreground;
    h_flex()
        .w_full()
        .items_center()
        .px_3()
        .py_1()
        .gap_2()
        .child(connection_col(px(108.), "进程", muted_fg, None))
        .child(connection_col_flex("目标", muted_fg, None))
        .child(connection_col(px(52.), "协议", muted_fg, None))
        .child(connection_col(px(168.), "走向", muted_fg, None))
        .child(connection_col(px(72.), "上传", muted_fg, None))
        .child(connection_col(px(72.), "下载", muted_fg, None))
        .child(connection_col(px(80.), "时长", muted_fg, None))
        .child(div().w(px(56.)))
}

fn connection_col(
    width: Pixels,
    text: impl Into<String>,
    color: Hsla,
    mono: Option<SharedString>,
) -> impl IntoElement {
    div()
        .w(width)
        .min_w(width)
        .text_xs()
        .text_color(color)
        .when_some(mono, |this, family| this.font_family(family))
        .child(text.into())
}

fn connection_col_flex(
    text: impl Into<String>,
    color: Hsla,
    mono: Option<SharedString>,
) -> impl IntoElement {
    div()
        .flex_1()
        .min_w(px(96.))
        .text_xs()
        .text_color(color)
        .when_some(mono, |this, family| this.font_family(family))
        .child(text.into())
}

fn render_connection_row(
    entity: Entity<AppView>,
    theme: &Theme,
    conn: &controller::LiveConnection,
) -> impl IntoElement {
    let id = conn.id.clone();
    let muted = theme.muted;
    let muted_fg = theme.muted_foreground;
    let fg = theme.foreground;
    let mono = theme.mono_font_family.clone();
    let up = controller::format_bytes(conn.upload);
    let down = controller::format_bytes(conn.download);
    h_flex()
        .id(SharedString::from(format!("conn-{id}")))
        .w_full()
        .items_center()
        .px_3()
        .py_2()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .bg(theme.group_box)
        .hover(move |style| style.bg(muted))
        .child(connection_col(px(108.), conn.process.clone(), fg, None))
        .child(connection_col_flex(
            conn.destination.clone(),
            muted_fg,
            Some(mono.clone()),
        ))
        .child(connection_col(px(52.), conn.network.clone(), muted_fg, None))
        .child(connection_col(px(168.), conn.chain.clone(), fg, None))
        .child(connection_col(px(72.), up, muted_fg, Some(mono.clone())))
        .child(connection_col(px(72.), down, muted_fg, Some(mono)))
        .child(connection_col(
            px(80.),
            conn.duration.clone(),
            muted_fg,
            None,
        ))
        .child({
            let entity = entity.clone();
            Button::new(SharedString::from(format!("close-conn-{id}")))
                .small()
                .danger()
                .label("关闭")
                .on_click(move |_, _, app| {
                    let id = id.clone();
                    entity.update(app, |this, cx| {
                        this.traffic.connections.retain(|conn| conn.id != id);
                        let port = this.strategy.mixed_port;
                        let close_id = id.clone();
                        cx.background_executor()
                            .spawn(async move {
                                if let Err(err) = controller::close_one(port, &close_id) {
                                    log::debug("ui", format!("close connection failed: {err:#}"));
                                }
                            })
                            .detach();
                        cx.notify();
                    });
                })
        })
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

fn render_rule_set_card(
    entity: Entity<AppView>,
    theme: &Theme,
    index: usize,
    total: usize,
    selected: bool,
    set: &RuleSet,
    via_choices: Vec<ViaChoice>,
    accent: Hsla,
) -> impl IntoElement {
    let id = set.id.clone();
    let via = set.via.clone();
    let can_up = index > 0;
    let can_down = index + 1 < total;
    let muted = theme.muted;
    let muted_fg = theme.muted_foreground;
    const CHIP_LIMIT: usize = 10;
    let extra = set.matchers.len().saturating_sub(CHIP_LIMIT);
    v_flex()
        .id(SharedString::from(format!("rule-card-{id}")))
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
                    this.open_rule_dialog(Some(&id), window, cx);
                    cx.notify();
                });
            }
        })
        .context_menu({
            let entity = entity.clone();
            let id = id.clone();
            let via = via.clone();
            move |menu, window, cx| {
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
                            this.open_rule_dialog(Some(&edit_id), window, cx);
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
                            let entity = entity.clone();
                            let id = id.clone();
                            via_menu(menu, &choices, &via_current, move |app, value| {
                                entity.update(app, |this, cx| {
                                    this.set_selected_rule_via(&id, &value);
                                    cx.notify();
                                });
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
            }
        })
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_fg)
                                .child(format!("{}", index + 1)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(set.name.clone()),
                        )
                        .child(pill(theme, &via_label(&via), accent)),
                )
                .child({
                    let entity = entity.clone();
                    let del_id = id.clone();
                    Button::new(SharedString::from(format!("del-rule-{del_id}")))
                        .small()
                        .danger()
                        .label("删除")
                        .on_click(move |_, window, app| {
                            app.stop_propagation();
                            entity.update(app, |this, cx| {
                                this.remove_selected_rule(&del_id, window, cx);
                                cx.notify();
                            });
                        })
                }),
        )
        .child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .children(set.matchers.iter().take(CHIP_LIMIT).map(|matcher| {
                    outline_pill(
                        theme,
                        &format!("{} {}", matcher.kind_label(), matcher.display_value()),
                    )
                }))
                .when(extra > 0, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(format!("+{extra}")),
                    )
                })
                .when(set.matchers.is_empty(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child("没有匹配项"),
                    )
                }),
        )
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
