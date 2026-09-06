use std::collections::{HashMap, HashSet};
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
    h_flex, v_flex, ActiveTheme, Disableable, IconName, Root, Selectable, Sizable, StyledExt,
    Theme, TitleBar, WindowExt,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use myproxy::catalog::{self, Catalog};
use myproxy::controller::{self, LiveGroup, LiveReach, PublishedLive, TrafficSnapshot};
use myproxy::log;
use myproxy::strategy::{join_list, parse_list, Group, Matcher, RuleSet, Strategy};
use myproxy::supervisor::Supervisor;
use myproxy::updates::{self, UpdateChannel};

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
    selected: String,
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
            .map(|g| match g.kind.as_str() {
                "url-test" => "url-test",
                "fallback" => "fallback",
                _ => "select",
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
        let selected = existing
            .as_ref()
            .map(|g| g.selected.clone())
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
            selected,
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
            selected: self.selected.clone(),
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
            let previous = parent.strategy.clone();
            if let Some(id) = &edit_id {
                if !parent.strategy.update_group(id, next) {
                    return Err("保存失败：节点组已不存在。".to_string());
                }
            } else {
                next.id = uuid::Uuid::new_v4().to_string();
                parent.strategy.add_group(next);
            }
            if parent.persist_and_apply(cx) {
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
                parent.strategy = previous;
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

    fn move_pin(&mut self, name: &str, delta: i32) {
        let Some(ix) = self.include.iter().position(|n| n == name) else {
            return;
        };
        let to = ix as i32 + delta;
        if to < 0 || to >= self.include.len() as i32 {
            return;
        }
        let item = self.include.remove(ix);
        self.include.insert(to as usize, item);
        self.notice = "已调整钉住优先度。保存后生效。".into();
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
                    }),
            )
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .flex_wrap()
                    .child(div().text_xs().text_color(muted_fg).child("策略"))
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
                                    .label("手动选择")
                                    .selected(self.kind != "fallback" && self.kind != "url-test"),
                            )
                            .child(
                                Button::new("group-kind-fallback")
                                    .small()
                                    .label("自动切换（不可用则下一个）")
                                    .selected(self.kind == "fallback"),
                            )
                            .child(
                                Button::new("group-kind-url")
                                    .small()
                                    .label("延迟最低")
                                    .selected(self.kind == "url-test"),
                            );
                        group.on_click(move |ixs, _, app| {
                            let Some(&ix) = ixs.first() else {
                                return;
                            };
                            entity.update(app, |this, cx| {
                                this.kind = match ix {
                                    1 => "fallback".into(),
                                    2 => "url-test".into(),
                                    _ => "select".into(),
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
                    .child(format!(
                        "预览 · {} 个节点 · {} · {}",
                        members.len(),
                        draft.kind_setting_label(),
                        draft.policy_label()
                    )),
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
                    .children(members.iter().take(PREVIEW_LIMIT).enumerate().map(|(ix, name)| {
                        render_member_row(
                            entity.clone(),
                            &theme,
                            name,
                            self.include.iter().any(|n| n == name),
                            false,
                            (self.kind == "fallback").then_some(ix + 1),
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
                            .map(|name| {
                                render_member_row(entity.clone(), &theme, name, false, true, None)
                            }),
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
        fallback_via: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let via = existing
            .as_ref()
            .map(|s| s.via.clone())
            .unwrap_or(fallback_via);
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
            let previous = parent.strategy.clone();
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
            if parent.persist_and_apply(cx) {
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
                parent.strategy = previous;
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

enum LivePoll {
    Down,
    Ready(Result<controller::LiveSnapshot, String>),
}

pub struct AppView {
    page: Page,
    strategy: Strategy,
    saved: Strategy,
    applied: Strategy,
    catalog: Catalog,
    status: String,
    connected: bool,
    busy: bool,
    cli_installed: bool,
    external_change_pending: bool,
    strategy_stamp: Option<SystemTime>,
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
    pending_filter_input: Option<String>,
    pending_port_input: Option<String>,
    appearance: Appearance,
    _appearance_observer: Subscription,
    traffic: TrafficSnapshot,
    traffic_up: u64,
    traffic_down: u64,
    traffic_has_rate: bool,
    traffic_error: Option<String>,
    traffic_prev: Option<(Instant, u64, u64)>,
    proxy_groups: Vec<LiveGroup>,
    proxy_error: Option<String>,
    live_ok: bool,
    live_error: Option<String>,
    live_inflight: bool,
    delays: HashMap<String, u32>,
    delaying: HashSet<String>,
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
        let initial_strategy_stamp = strategy_path.as_deref().and_then(file_stamp);
        cx.spawn(async move |this, cx| {
            let mut catalog_stamp = catalog_path.as_deref().and_then(file_stamp);
            let mut fail_streak: u32 = 0;
            loop {
                let wait_ms = this
                    .update(cx, |this, _| {
                        if this.live_inflight {
                            return 200;
                        }
                        let base = if this.page == Page::Connections {
                            500
                        } else {
                            1500
                        };
                        if fail_streak == 0 {
                            base
                        } else {
                            base.saturating_mul(1 << fail_streak.min(3)).min(8000)
                        }
                    })
                    .unwrap_or(1500);
                cx.background_executor()
                    .timer(Duration::from_millis(wait_ms))
                    .await;

                let request = this
                    .update(cx, |this, _| {
                        if this.busy || this.live_inflight {
                            return None;
                        }
                        this.live_inflight = true;
                        Some((this.strategy.mixed_port, this.supervisor.clone()))
                    })
                    .ok()
                    .flatten();
                if let Some((port, supervisor)) = request {
                    let outcome = cx
                        .background_executor()
                        .spawn(async move {
                            if !supervisor.is_running() {
                                return LivePoll::Down;
                            }
                            LivePoll::Ready(
                                controller::fetch_live(port).map_err(|err| err.to_string()),
                            )
                        })
                        .await;
                    fail_streak = match &outcome {
                        LivePoll::Ready(Err(_)) => fail_streak.saturating_add(1).min(3),
                        _ => 0,
                    };
                    if this
                        .update(cx, |this, cx| {
                            this.live_inflight = false;
                            this.apply_live_poll(outcome);
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
                        let editing = this.group_modal_open || this.rule_modal_open;
                        if !this.busy {
                            if let Some(path) = strategy_path.as_deref() {
                                let stamp = file_stamp(path);
                                if stamp != this.strategy_stamp {
                                    if let Ok(strategy) = Strategy::load() {
                                        this.strategy_stamp = stamp;
                                        log::set_developer(strategy.developer_mode);
                                        let local_inputs_dirty = this.port_input.read(cx).value().trim()
                                            != this.strategy.mixed_port.to_string()
                                            || this.filter_input.read(cx).value().to_string()
                                                != this.strategy.exclude_filter;
                                        if editing || this.strategy != this.saved || local_inputs_dirty {
                                            this.external_change_pending = true;
                                            this.status = if editing {
                                                "编辑期间检测到外部策略变更。请取消编辑后再决定是否应用覆盖。"
                                            } else {
                                                "检测到外部策略变更。点击「覆盖并应用」保留本地配置，或重新打开窗口放弃本地修改。"
                                            }
                                            .into();
                                        } else {
                                            this.strategy = strategy.clone();
                                            this.saved = strategy.clone();
                                            this.applied.update_channel = strategy.update_channel;
                                            crate::sparkle::set_channel(strategy.update_channel.unwrap_or_default());
                                            this.pending_port_input =
                                                Some(strategy.mixed_port.to_string());
                                            this.pending_filter_input =
                                                Some(strategy.exclude_filter.clone());
                                            this.external_change_pending = false;
                                        }
                                        dirty = true;
                                        log::debug("ui", "reload strategy.json");
                                    }
                                }
                            }
                            if let Some(path) = catalog_path.as_deref() {
                                let stamp = file_stamp(path);
                                if stamp != catalog_stamp {
                                    if let Ok(catalog) = Catalog::load() {
                                        catalog_stamp = stamp;
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

        let supervisor = Supervisor::shared();
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
        let this = Self {
            page: initial_page(),
            status: if connected {
                "正在读取核心状态…".into()
            } else {
                "策略已加载。在总览连接；改端口或过滤器后点「应用」。".into()
            },
            connected,
            busy: false,
            cli_installed: myproxy::cli_install::is_installed(),
            external_change_pending: false,
            strategy_stamp: initial_strategy_stamp,
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
            strategy: strategy.clone(),
            saved: strategy.clone(),
            applied: strategy,
            catalog,
            appearance,
            _appearance_observer: appearance_observer,
            traffic: TrafficSnapshot::default(),
            traffic_up: 0,
            traffic_down: 0,
            traffic_has_rate: false,
            traffic_error: None,
            traffic_prev: None,
            proxy_groups: Vec::new(),
            proxy_error: None,
            live_ok: false,
            live_error: None,
            live_inflight: false,
            delays: HashMap::new(),
            delaying: HashSet::new(),
            pending_port_input: None,
            pending_filter_input: None,
        };
        if crate::onboard::should_prompt() {
            cx.defer_in(window, |_this, window, cx| {
                let entity = cx.entity();
                crate::onboard::open(window, cx, move |result, cx| {
                    entity.update(cx, |this, cx| {
                        match result {
                            Ok(path) => {
                                this.cli_installed = true;
                                this.status = format!("命令行工具已安装：{}", path.display());
                            }
                            Err(error) => {
                                this.status = format!("命令行工具安装失败：{error}");
                            }
                        }
                        cx.notify();
                    });
                });
            });
        }
        this
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
        self.persist_with_override(false)
    }

    fn persist_with_override(&mut self, overwrite_external: bool) -> bool {
        if self.busy {
            self.status = "正在处理上一项操作。".into();
            return false;
        }
        // Check at the write boundary too: the watcher may not have polled yet.
        let disk = match myproxy::paths::strategy_path()
            .and_then(|path| myproxy::strategy::load_from(&path))
        {
            Ok(strategy) => strategy,
            Err(err) => {
                self.status = format!("无法读取磁盘策略，未保存：{err}");
                return false;
            }
        };
        if !overwrite_external && (self.external_change_pending || disk != self.saved) {
            self.external_change_pending = true;
            self.status = "检测到外部策略变更。点击「覆盖并应用」保留本地配置，或重新打开窗口放弃本地修改。".into();
            return false;
        }
        match self.strategy.save() {
            Ok(()) => {
                if let Ok(path) = myproxy::paths::strategy_path() {
                    self.strategy_stamp = file_stamp(&path);
                }
                self.saved = self.strategy.clone();
                self.applied.update_channel = self.strategy.update_channel;
                crate::sparkle::set_channel(self.strategy.update_channel.unwrap_or_default());
                self.external_change_pending = false;
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

    fn persist_and_apply(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy {
            self.status = "正在处理上一项操作。".into();
            return false;
        }
        if !self.persist() {
            return false;
        }
        self.start_apply(cx)
    }

    fn start_apply(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy {
            self.status = "正在处理上一项操作。".into();
            return false;
        }
        let strategy = self.strategy.clone();
        let supervisor = self.supervisor.clone();
        self.busy = true;
        self.status = "正在应用策略…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let apply_strategy = strategy.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    supervisor
                        .apply(&apply_strategy)
                        .map_err(|err| err.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(cat) => {
                        this.catalog = cat;
                        if this.strategy == strategy {
                            this.mark_applied();
                        }
                        this.status = format!(
                            "已编译 {} 个节点，排除 {}。{}Mixed {}。",
                            this.catalog.nodes.len(),
                            this.catalog.excluded.len(),
                            if this.strategy.system_extension {
                                "系统接管 · "
                            } else {
                                ""
                            },
                            this.strategy.mixed_port
                        );
                        if this.connected {
                            this.refresh_live(cx);
                        } else {
                            this.live_ok = false;
                            this.publish_session();
                        }
                    }
                    Err(err) => {
                        log::error("ui", format!("apply failed: {err}"));
                        this.status = format!("应用失败: {err}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        true
    }

    fn is_dirty(&self) -> bool {
        let mut current = self.strategy.clone();
        let mut applied = self.applied.clone();
        for group in &mut current.groups {
            group.selected.clear();
        }
        for group in &mut applied.groups {
            group.selected.clear();
        }
        current != applied
    }

    fn mark_applied(&mut self) {
        self.applied = self.strategy.clone();
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

    fn clear_live(&mut self) -> bool {
        let dirty = self.clear_traffic()
            || !self.proxy_groups.is_empty()
            || self.proxy_error.is_some()
            || self.live_ok
            || self.live_error.is_some()
            || !self.delays.is_empty()
            || !self.delaying.is_empty();
        self.proxy_groups.clear();
        self.proxy_error = None;
        self.live_ok = false;
        self.live_error = None;
        self.delays.clear();
        self.delaying.clear();
        dirty
    }

    fn publish_session(&self) {
        let reach = if !self.connected {
            LiveReach::Down
        } else if self.live_ok {
            LiveReach::Ok
        } else if self.live_error.is_some() {
            LiveReach::Unreachable
        } else {
            LiveReach::Unknown
        };
        controller::publish_live(PublishedLive {
            reach,
            proxy_now: self.live_now("PROXY").unwrap_or("").to_string(),
        });
    }

    fn apply_live_poll(&mut self, poll: LivePoll) {
        match poll {
            LivePoll::Down => {
                self.connected = false;
                self.clear_live();
                self.publish_session();
            }
            LivePoll::Ready(Ok(snap)) => {
                self.connected = true;
                self.live_ok = true;
                self.live_error = None;
                self.apply_traffic(Ok(snap.traffic));
                self.apply_proxies(Ok(snap.groups));
                self.publish_session();
            }
            LivePoll::Ready(Err(err)) => {
                log::debug("ui", format!("live poll failed: {err}"));
                self.connected = true;
                self.live_ok = false;
                self.live_error = Some("读不到核心状态。".into());
                self.traffic = TrafficSnapshot::default();
                self.traffic_up = 0;
                self.traffic_down = 0;
                self.traffic_has_rate = false;
                self.traffic_prev = None;
                self.proxy_groups.clear();
                self.traffic_error = self.live_error.clone();
                self.proxy_error = self.live_error.clone();
                self.publish_session();
            }
        }
    }

    fn session_healthy(&self) -> bool {
        self.connected && self.live_ok
    }

    fn session_title(&self) -> String {
        if self.busy {
            return self.status.clone();
        }
        if !self.connected {
            return "未连接".into();
        }
        if self.live_ok {
            if self.traffic_has_rate {
                return format!(
                    "已连接 · ↑{} · ↓{}",
                    controller::format_rate(self.traffic_up),
                    controller::format_rate(self.traffic_down)
                );
            }
            return "已连接".into();
        }
        if self.live_error.is_some() {
            "核心无响应".into()
        } else {
            "正在读取核心…".into()
        }
    }

    fn session_headline(&self) -> &'static str {
        if !self.connected {
            "未连接"
        } else if self.live_ok {
            "已连接"
        } else if self.live_error.is_some() {
            "核心无响应"
        } else {
            "正在读取核心…"
        }
    }

    fn live_metric(&self, value: String) -> String {
        if self.session_healthy() {
            value
        } else if self.connected && self.live_error.is_some() {
            "读不到".into()
        } else {
            "—".into()
        }
    }

    fn overview_proxy_label(&self) -> String {
        if !self.connected {
            return "未连接".into();
        }
        if !self.live_ok {
            return if self.live_error.is_some() {
                "读不到".into()
            } else {
                "—".into()
            };
        }
        self.live_now("PROXY")
            .map(str::to_string)
            .or_else(|| {
                self.strategy
                    .groups
                    .iter()
                    .find(|group| {
                        group.name == "PROXY" || group.name.eq_ignore_ascii_case("default")
                    })
                    .map(|group| group.selected.clone())
                    .filter(|name| !name.is_empty())
            })
            .unwrap_or_else(|| "—".into())
    }

    fn apply_proxies(&mut self, result: Result<Vec<LiveGroup>, String>) {
        match result {
            Ok(mut groups) => {
                for group in &mut groups {
                    for member in &mut group.members {
                        if let Some(delay) = self.delays.get(&member.name) {
                            member.delay = Some(*delay);
                        }
                    }
                }
                self.proxy_groups = groups;
                self.proxy_error = None;
            }
            Err(err) => {
                log::debug("ui", format!("proxies poll failed: {err}"));
                self.proxy_error = Some("暂时读不到节点组状态。".into());
            }
        }
    }

    fn live_now(&self, name: &str) -> Option<&str> {
        self.proxy_groups
            .iter()
            .find(|group| group.name == name)
            .map(|group| group.now.as_str())
            .filter(|now| !now.is_empty())
    }

    fn refresh_live(&mut self, cx: &mut Context<Self>) {
        if !self.connected {
            self.clear_live();
            self.publish_session();
            return;
        }
        let port = self.strategy.mixed_port;
        self.live_inflight = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { controller::fetch_live(port).map_err(|err| err.to_string()) })
                .await;
            this.update(cx, |this, cx| {
                this.live_inflight = false;
                this.apply_live_poll(LivePoll::Ready(result));
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                if this.busy {
                    this.status = "正在处理上一项操作。".into();
                    cx.notify();
                    return;
                }
                let Ok(port) = this.port_input.read(cx).value().trim().parse::<u16>() else {
                    this.status = "端口无效。".into();
                    cx.notify();
                    return;
                };
                this.strategy.mixed_port = port;
                this.strategy.exclude_filter = this.filter_input.read(cx).value().to_string();
                if this.persist_with_override(this.external_change_pending) {
                    this.start_apply(cx);
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
                this.start_connect(cx);
            });
        }
    }

    fn start_connect(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            self.status = "正在处理上一项操作。".into();
            cx.notify();
            return;
        }
        let connect = !self.connected;
        if connect && !self.persist() {
            cx.notify();
            return;
        }
        let strategy = self.strategy.clone();
        let supervisor = self.supervisor.clone();
        self.busy = true;
        self.status = if connect {
            "正在连接…".into()
        } else {
            "正在断开…".into()
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let connect_strategy = strategy.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    if connect {
                        supervisor
                            .connect(&connect_strategy)
                            .map(|_| Some(Catalog::load().unwrap_or_default()))
                            .map_err(|err| err.to_string())
                    } else {
                        supervisor
                            .disconnect()
                            .map(|_| None)
                            .map_err(|err| err.to_string())
                    }
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(Some(catalog)) => {
                        this.connected = true;
                        this.live_ok = false;
                        this.live_error = None;
                        this.catalog = catalog;
                        if this.strategy == strategy {
                            this.mark_applied();
                        }
                        this.status = format!(
                            "已连接 {}127.0.0.1:{}（HTTP + SOCKS5）",
                            if this.strategy.system_extension {
                                "系统接管 · "
                            } else {
                                ""
                            },
                            this.strategy.mixed_port
                        );
                        this.publish_session();
                        this.refresh_live(cx);
                    }
                    Ok(None) => {
                        this.connected = false;
                        this.clear_live();
                        this.publish_session();
                        this.status = "已断开。".into();
                    }
                    Err(err) => {
                        log::error("ui", format!("connection operation failed: {err}"));
                        this.status = format!("操作失败: {err}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let fallback_via = default_via(&self.strategy);
        let editor = cx.new(|cx| RuleSetEditor::new(parent.clone(), existing, fallback_via, window, cx));
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

    fn set_selected_rule_via(&mut self, id: &str, via: &str, cx: &mut Context<Self>) {
        if !self.strategy.set_rule_via(id, via.to_string()) {
            self.status = "改走向失败。".into();
            return;
        }
        self.persist_and_apply(cx);
    }

    fn move_selected_rule(&mut self, id: &str, delta: i32, cx: &mut Context<Self>) {
        if !self.strategy.move_rule(id, delta) {
            return;
        }
        self.persist_and_apply(cx);
    }

    fn remove_selected_rule(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let editing = self.rule_edit_id.as_deref() == Some(id);
        if !self.strategy.remove_rule(id) {
            return;
        }
        if self.persist_and_apply(cx) {
            if editing {
                self.rule_modal_open = false;
                self.rule_edit_id = None;
                window.close_dialog(cx);
            }
            self.status = "已删除规则。".into();
        }
    }

    fn group_now<'a>(&'a self, group: &'a Group) -> &'a str {
        self.live_now(&group.name)
            .or_else(|| {
                if group.selected.is_empty() {
                    None
                } else {
                    Some(group.selected.as_str())
                }
            })
            .unwrap_or("—")
    }

    fn group_member_names(&self, group: &Group) -> Vec<String> {
        if let Some(live) = self
            .proxy_groups
            .iter()
            .find(|live| live.name == group.name)
        {
            return live.members.iter().map(|member| member.name.clone()).collect();
        }
        catalog::resolve_group_members(group, &self.catalog)
    }

    fn member_delay(&self, name: &str) -> Option<u32> {
        self.delays.get(name).copied().or_else(|| {
            self.proxy_groups
                .iter()
                .flat_map(|group| group.members.iter())
                .find(|member| member.name == name)
                .and_then(|member| member.delay)
        })
    }

    fn select_group_member(&mut self, group_id: &str, node: &str, cx: &mut Context<Self>) {
        if !self.strategy.set_group_selected(group_id, node.to_string()) {
            self.status = "只能在手动选择组里点选节点。".into();
            cx.notify();
            return;
        }
        let group_name = self
            .strategy
            .groups
            .iter()
            .find(|group| group.id == group_id || group.name == group_id)
            .map(|group| group.name.clone())
            .unwrap_or_default();
        if let Some(applied) = self
            .applied
            .groups
            .iter_mut()
            .find(|group| group.id == group_id || group.name == group_id)
        {
            applied.selected = node.to_string();
        }
        if let Some(live) = self
            .proxy_groups
            .iter_mut()
            .find(|group| group.name == group_name)
        {
            live.now = node.to_string();
        }
        if self.persist() {
            self.status = format!("已切换到 {node}");
        }
        if self.connected && !group_name.is_empty() {
            let port = self.strategy.mixed_port;
            let group_name = group_name.clone();
            let node = node.to_string();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        controller::select_proxy(port, &group_name, &node)
                            .map_err(|err| err.to_string())
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if let Err(err) = result {
                        this.status = format!("切换失败: {err}");
                    }
                    this.refresh_live(cx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn start_group_delay(&mut self, group_name: &str, cx: &mut Context<Self>) {
        if !self.connected {
            self.status = "连接后再测延迟。".into();
            cx.notify();
            return;
        }
        if !self.delaying.insert(group_name.to_string()) {
            return;
        }
        self.status = format!("正在测 {group_name} 延迟…");
        let port = self.strategy.mixed_port;
        let name = group_name.to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let probe = name.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    controller::test_group_delay(port, &probe).map_err(|err| err.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.delaying.remove(&name);
                match result {
                    Ok(map) => {
                        for (member, delay) in map {
                            if member.is_empty() {
                                if let Some(now) = this.live_now(&name).map(str::to_string) {
                                    this.delays.insert(now, delay);
                                }
                            } else {
                                this.delays.insert(member, delay);
                            }
                        }
                        for group in &mut this.proxy_groups {
                            for member in &mut group.members {
                                if let Some(delay) = this.delays.get(&member.name) {
                                    member.delay = Some(*delay);
                                }
                            }
                        }
                        this.status = format!("已更新 {name} 延迟。");
                    }
                    Err(err) => {
                        this.status = format!("测延迟失败: {err}");
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(value) = self.pending_port_input.take() {
            self.port_input
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        if let Some(value) = self.pending_filter_input.take() {
            self.filter_input
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
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
        let healthy = self.session_healthy();
        let busy = self.busy;
        let live_label = self.session_title();
        let label_color = if !busy && self.connected && self.live_error.is_some() {
            theme.warning
        } else {
            theme.muted_foreground
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
                        .children(updates::build_badge().map(|label| pill(theme, label, theme.warning))),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(status_dot(theme, healthy))
                        .child(
                            div()
                                .text_xs()
                                .text_color(label_color)
                                .child(live_label),
                        )
                        .when(self.is_dirty() || self.external_change_pending, |this| {
                            this.child(
                                Button::new("apply")
                                    .small()
                                    .disabled(busy)
                                    .label(if busy {
                                        "处理中…"
                                    } else if self.external_change_pending {
                                        "覆盖并应用"
                                    } else {
                                        "应用"
                                    })
                                    .on_click(self.on_apply(cx)),
                            )
                        }),
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
                    .text_color(if self.status.contains("失败") {
                        theme.warning
                    } else {
                        theme.muted_foreground
                    })
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
                        Page::Overview => self.overview(cx, theme).into_any_element(),
                        Page::Subscriptions => self.subscriptions(cx, theme).into_any_element(),
                        Page::Groups => self.groups(cx, theme).into_any_element(),
                        Page::Settings => self.settings(cx, theme).into_any_element(),
                        Page::Rules | Page::Connections => unreachable!(),
                    })
                    .into_any_element(),
            })
    }

    fn overview(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let connected = self.connected;
        let busy = self.busy;
        let mut connect = Button::new("hero-connect").large();
        connect = if busy {
            connect.label(if connected { "正在断开…" } else { "正在连接…" })
        } else if connected {
            connect.danger().label("断开")
        } else {
            connect.primary().label("连接")
        };
        let up = self.live_metric(if self.session_healthy() && self.traffic_has_rate {
            controller::format_rate(self.traffic_up)
        } else {
            "—".into()
        });
        let down = self.live_metric(if self.session_healthy() && self.traffic_has_rate {
            controller::format_rate(self.traffic_down)
        } else {
            "—".into()
        });
        let conns = self.live_metric(if self.session_healthy() {
            self.traffic.connections.len().to_string()
        } else {
            "—".into()
        });
        let now = self.overview_proxy_label();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "总览",
                if self.strategy.system_extension {
                    "系统接管已打开。应用不用自己填代理。请在系统设置里允许扩展。"
                } else {
                    "未打开系统接管时，只有自己填了代理的应用会进规则。"
                },
            ))
            .child(
                h_flex()
                    .w_full()
                    .p_5()
                    .gap_4()
                    .items_center()
                    .justify_between()
                    .rounded(theme.radius)
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.group_box)
                    .child(
                        v_flex()
                            .gap(px(4.))
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(status_dot(theme, self.session_healthy()))
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_semibold()
                                            .child(self.session_headline()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if connected {
                                        if self.strategy.system_extension {
                                            format!(
                                                "系统接管中 · 应用无需填代理 · Mixed 127.0.0.1:{} 仍可用",
                                                self.strategy.mixed_port
                                            )
                                        } else {
                                            format!(
                                                "仅 Mixed 127.0.0.1:{} · 未填代理的应用不会进规则",
                                                self.strategy.mixed_port
                                            )
                                        }
                                    } else if self.strategy.system_extension {
                                        "下次连接会请求系统扩展。请在系统设置 › 通用 › 登录项与扩展 › 网络扩展 里允许 myproxy。".into()
                                    } else {
                                        "连接后，自己填了代理的应用会进规则。要拦截其他应用，先打开设置里的系统接管。".into()
                                    }),
                            ),
                    )
                    .child(
                        connect
                            .h(px(48.))
                            .px_8()
                            .min_w(px(132.))
                            .disabled(busy)
                            .on_click(self.on_connect(cx)),
                    ),
            )
            .child(
                h_flex()
                    .gap_3()
                    .child(metric(
                        theme,
                        "状态",
                        self.session_headline(),
                    ))
                    .child(metric(theme, "上传", &up))
                    .child(metric(theme, "下载", &down))
                    .child(metric(theme, "连接数", &conns))
                    .child(metric(theme, "PROXY", &now))
                    .child(metric(
                        theme,
                        "系统接管",
                        if self.strategy.system_extension { "开" } else { "关" },
                    ))
                    .child(metric(
                        theme,
                        "Mixed",
                        &format!("127.0.0.1:{}", self.strategy.mixed_port),
                    ))
                    .child(metric(
                        theme,
                        "节点",
                        &format!(
                            "{} kept / {} excluded",
                            self.catalog.nodes.len(),
                            self.catalog.excluded.len()
                        ),
                    )),
            )
            .when(self.live_error.is_some(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(self.live_error.clone().unwrap_or_default()),
                )
            })
    }

    fn connections(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let connected = self.connected;
        let healthy = self.session_healthy();
        let muted_fg = theme.muted_foreground;
        let up = self.live_metric(if healthy && self.traffic_has_rate {
            controller::format_rate(self.traffic_up)
        } else {
            "—".into()
        });
        let down = self.live_metric(if healthy && self.traffic_has_rate {
            controller::format_rate(self.traffic_down)
        } else {
            "—".into()
        });
        let count = self.live_metric(if healthy {
            self.traffic.connections.len().to_string()
        } else {
            "—".into()
        });
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
                if self.strategy.system_extension {
                    "系统接管打开后，未自己填代理的应用也会出现在这里。主动指定代理的连接仍会列出。"
                } else {
                    "当前只看到主动指定代理的连接。要拦截其他应用，先打开设置里的系统接管。"
                },
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(metric(
                        theme,
                        "状态",
                        self.session_headline(),
                    ))
                    .child(metric(theme, "Mixed 端口", &mixed))
                    .child(metric(theme, "上传", &up))
                    .child(metric(theme, "下载", &down))
                    .child(metric(theme, "连接数", &count)),
            )
            .when(
                self.live_error.is_some()
                    || self.traffic_error.is_some()
                    || (healthy && !self.traffic.connections.is_empty()),
                |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if self.live_error.is_some() {
                                        theme.warning
                                    } else {
                                        muted_fg
                                    })
                                    .child(
                                        self.live_error
                                            .clone()
                                            .or_else(|| self.traffic_error.clone())
                                            .unwrap_or_default(),
                                    ),
                            )
                            .when(healthy && !self.traffic.connections.is_empty(), |this| {
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
                healthy && self.traffic.connections.is_empty(),
                |this| {
                    this.child(empty_hint(
                        theme,
                        "暂时没有连接。只有进入 Mixed 端口的流量会出现在这里。",
                    ))
                },
            )
            .when(
                connected && !healthy && self.traffic.connections.is_empty(),
                |this| {
                    this.child(empty_hint(
                        theme,
                        "核心在跑，但读不到连接列表。不是没有流量，是控制器没响应。",
                    ))
                },
            )
            .when(healthy && !self.traffic.connections.is_empty(), |this| {
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
                "点节点切换当前出口；点卡片空白处编辑匹配条件。来源 ∩ 名称含（或，支持 * ?）∪ 钉住 − 排除。",
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
            .when(
                self.connected && (self.live_error.is_some() || self.proxy_error.is_some()),
                |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.warning)
                            .child(
                                self.live_error
                                    .clone()
                                    .or_else(|| self.proxy_error.clone())
                                    .unwrap_or_default(),
                            ),
                    )
                },
            )
            .children(self.strategy.groups.iter().map(|group| {
                let count = catalog::count_group_members(group, &self.catalog);
                let selected = self.group_edit_id.as_deref() == Some(group.id.as_str());
                let now = self.group_now(group).to_string();
                let members: Vec<(String, Option<u32>)> = self
                    .group_member_names(group)
                    .into_iter()
                    .map(|name| {
                        let delay = self.member_delay(&name);
                        (name, delay)
                    })
                    .collect();
                render_group_card(
                    entity.clone(),
                    theme,
                    group,
                    count,
                    selected,
                    accent,
                    &now,
                    &members,
                    group.kind == "select",
                    self.delaying.contains(&group.name),
                    self.connected,
                )
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
                "打开系统接管后，应用不用自己填代理。第一次会请你在系统设置里允许 myproxy。",
            ))
            .child(self.system_extension_panel(cx, theme))
            .child(panel(
                theme,
                "外观",
                v_flex().gap_3().child(self.appearance_row(cx, theme)),
            ))
            .child(panel(
                theme,
                "显式代理（Mixed）",
                v_flex()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("主动指定 HTTP/SOCKS 的客户端仍用这个端口。系统接管打开后，未填代理的应用也会进核心。"),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(div().text_sm().child("127.0.0.1"))
                            .child(div().w(px(100.)).child(Input::new(&self.port_input)))
                            .child({
                                let entity = entity.clone();
                                Button::new("save-port")
                                    .disabled(self.busy)
                                    .label("保存端口")
                                    .on_click(
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
                    ),
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
                        Button::new("save-filter")
                            .disabled(self.busy)
                            .primary()
                            .label("保存过滤器")
                            .on_click(
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
            .child(self.startup_panel(cx, theme))
            .child(self.cli_install_panel(cx, theme))
            .child(self.developer_panel(cx, theme))
    }

    fn cli_install_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let installed = self.cli_installed;
        panel(
            theme,
            "命令行工具",
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap(px(2.))
                                .child(div().text_sm().child("安装 myproxyctl"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("让 Agent 或终端直接使用 myproxyctl 配置代理。"),
                                ),
                        )
                        .child({
                            let entity = entity.clone();
                            let mut button = Button::new("install-cli").small();
                            button = if installed {
                                button.label("已安装，更新链接")
                            } else {
                                button.primary().label("安装")
                            };
                            button.disabled(self.busy).on_click(move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    match myproxy::cli_install::install() {
                                        Ok(path) => {
                                            this.cli_installed = true;
                                            this.status = format!("命令行工具已安装：{}", path.display());
                                        }
                                        Err(error) => {
                                            this.status = format!("命令行工具安装失败：{error:#}");
                                        }
                                    }
                                    cx.notify();
                                });
                            })
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "安装路径：{}",
                            myproxy::cli_install::destination()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "无法确定用户主目录".into())
                        )),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("官方 Agent skill，复制后交给 Cursor / Codex。"),
                        )
                        .child({
                            let entity = entity.clone();
                            Button::new("copy-cli-skill")
                                .small()
                                .label("复制 Agent skill")
                                .on_click(move |_, _, cx| {
                                    crate::onboard::copy_agent_skill(cx);
                                    entity.update(cx, |this, cx| {
                                        this.status = "已复制 Agent skill。".into();
                                        cx.notify();
                                    });
                                })
                        }),
                ),
        )
    }

    fn set_system_extension(&mut self, on: bool, cx: &mut Context<Self>) {
        self.strategy.system_extension = on;
        if on {
            self.strategy.tun = false;
        }
        if self.connected {
            self.persist_and_apply(cx);
        } else if self.persist() {
            self.status = if on {
                "已记录。下次连接会请求系统扩展，请在系统设置里允许 myproxy。".into()
            } else {
                "已关闭系统接管。".into()
            };
        }
    }

    fn set_tun(&mut self, on: bool, cx: &mut Context<Self>) {
        self.strategy.tun = on;
        if on {
            self.strategy.system_extension = false;
        }
        if self.connected {
            self.persist_and_apply(cx);
        } else if self.persist() {
            self.status = if on {
                "已记录。下次连接走 TUN；首次会要管理员密码。".into()
            } else {
                "已关闭 TUN。".into()
            };
        }
    }

    fn system_extension_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let on = self.strategy.system_extension;
        let tun_on = self.strategy.tun;
        panel(
            theme,
            "系统接管",
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap(px(2.))
                                .child(div().text_sm().child("拦截本机应用"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("按规则接管未自己填写代理的应用，不必再给每个客户端设端口。"),
                                ),
                        )
                        .child({
                            let mut toggle = Button::new("se-toggle").small();
                            toggle = if on {
                                toggle.danger().label("关闭")
                            } else {
                                toggle.primary().label("开启")
                            };
                            toggle.disabled(self.busy).on_click({
                                let entity = entity.clone();
                                move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.set_system_extension(
                                            !this.strategy.system_extension,
                                            cx,
                                        );
                                        cx.notify();
                                    });
                                }
                            })
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if on {
                            "已打开。连接后到「系统设置 › 通用 › 登录项与扩展 › 网络扩展」允许 myproxy。"
                        } else {
                            "关闭时，只有自己填了代理的应用会进规则。"
                        }),
                )
                .child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(
                            v_flex()
                                .gap(px(2.))
                                .child(div().text_sm().child("TUN"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("用虚拟网卡拦截流量，与系统接管不能同时开。一般保持关闭。"),
                                ),
                        )
                        .child({
                            let entity = entity.clone();
                            let mut toggle = Button::new("tun-toggle").small();
                            toggle = if tun_on {
                                toggle.danger().label("关闭")
                            } else {
                                toggle.primary().label("开启")
                            };
                            toggle.disabled(self.busy).on_click(move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    this.set_tun(!this.strategy.tun, cx);
                                    cx.notify();
                                });
                            })
                        }),
                ),
        )
    }

    fn startup_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let bundled = myproxy::login_item::is_bundled();
        panel(
            theme,
            "启动",
            v_flex()
                .gap_3()
                .child(self.flag_row(
                    entity.clone(),
                    theme,
                    "launch-at-login",
                    "开机默认启动",
                    "登录后自动打开 myproxy。需要安装为 .app。",
                    self.strategy.launch_at_login,
                    |this, cx| {
                        this.strategy.launch_at_login = !this.strategy.launch_at_login;
                        let sync_err = myproxy::login_item::sync(this.strategy.launch_at_login).err();
                        this.persist();
                        if let Some(err) = sync_err {
                            log::warn("login", format!("{err:#}"));
                            this.status = format!("已保存。开机启动未生效：{err}");
                        }
                        cx.notify();
                    },
                ))
                .when(!bundled, |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("当前不是 .app，登录项不会注册。安装到 /Applications/myproxy.app 后生效。"),
                    )
                })
                .child(self.flag_row(
                    entity.clone(),
                    theme,
                    "silent-launch",
                    "静默启动",
                    "启动时不显示主窗口，可从菜单栏打开。",
                    self.strategy.silent_launch,
                    |this, cx| {
                        this.strategy.silent_launch = !this.strategy.silent_launch;
                        this.persist();
                        cx.notify();
                    },
                ))
                .child(self.flag_row(
                    entity.clone(),
                    theme,
                    "lite-mode",
                    "轻量模式",
                    "不加载主界面，仅运行核心与菜单栏。点菜单栏图标可打开窗口。",
                    self.strategy.lite_mode,
                    |this, cx| {
                        this.strategy.lite_mode = !this.strategy.lite_mode;
                        this.persist();
                        cx.notify();
                    },
                ))
                .child(self.flag_row(
                    entity,
                    theme,
                    "connect-on-launch",
                    "启动时默认连接",
                    "启动后自动连接。",
                    self.strategy.connect_on_launch,
                    |this, cx| {
                        this.strategy.connect_on_launch = !this.strategy.connect_on_launch;
                        this.persist();
                        cx.notify();
                    },
                )),
        )
    }

    fn flag_row<F>(
        &self,
        entity: Entity<Self>,
        theme: &Theme,
        id: &'static str,
        title: &'static str,
        hint: &'static str,
        on: bool,
        apply: F,
    ) -> impl IntoElement
    where
        F: Fn(&mut Self, &mut Context<Self>) + 'static,
    {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .gap_4()
            .child(
                v_flex()
                    .gap(px(2.))
                    .child(div().text_sm().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(hint),
                    ),
            )
            .child({
                let mut toggle = Button::new(id).small();
                toggle = if on {
                    toggle.label("关闭")
                } else {
                    toggle.primary().label("开启")
                };
                toggle.disabled(self.busy).on_click(move |_, _, app| {
                    entity.update(app, |this, cx| apply(this, cx));
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
        let version = updates::VERSION;
        let channel = self.strategy.update_channel.unwrap_or_default();
        let hint = match channel {
            UpdateChannel::Prod => "仅接收正式版本。切回后，会在发布比当前版本更新的正式版时更新。",
            UpdateChannel::Nightly => "接收 main 分支的每日构建，可能包含尚未稳定的改动。",
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
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(div().text_sm().child("更新通道"))
                        .child({
                            let entity = entity.clone();
                            ButtonGroup::new("update-channel")
                                .compact()
                                .outline()
                                .small()
                                .child(Button::new("update-prod")
                                    .label(UpdateChannel::Prod.label())
                                    .selected(channel == UpdateChannel::Prod)
                                    .disabled(self.busy))
                                .child(Button::new("update-nightly")
                                    .label(UpdateChannel::Nightly.label())
                                    .selected(channel == UpdateChannel::Nightly)
                                    .disabled(self.busy))
                                .on_click(move |indices, _, app| {
                                    let next = match indices.first() {
                                        Some(0) => UpdateChannel::Prod,
                                        Some(1) => UpdateChannel::Nightly,
                                        _ => return,
                                    };
                                    entity.update(app, |this, cx| {
                                        let previous = this.strategy.update_channel;
                                        this.strategy.update_channel = Some(next);
                                        if this.persist() {
                                            this.status = format!("更新通道已切换为{}。", next.label());
                                        } else {
                                            this.strategy.update_channel = previous;
                                        }
                                        cx.notify();
                                    });
                                })
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(hint),
                )
                .when(!crate::sparkle::available(), |this| {
                    this.child(div().text_xs().text_color(theme.muted_foreground)
                        .child("此开发构建不支持应用内更新。安装发布版后可按所选通道更新。"))
                })
                .child({
                    let entity = entity.clone();
                    Button::new("check-updates")
                        .label("检查更新")
                        .disabled(!crate::sparkle::available())
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
    rank: Option<usize>,
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
        .when_some(rank, |this, rank| {
            this.child(
                div()
                    .w(px(22.))
                    .text_xs()
                    .text_color(muted_fg)
                    .child(format!("{rank}")),
            )
        })
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
            let up_entity = entity.clone();
            let up_name = name_owned.clone();
            let down_entity = entity.clone();
            let down_name = name_owned.clone();
            let unpin_entity = entity.clone();
            let unpin_name = name_owned.clone();
            this.child(
                Button::new(SharedString::from(format!("pin-up-{up_name}")))
                    .small()
                    .label("上移")
                    .on_click(move |_, _, app| {
                        up_entity.update(app, |this, cx| {
                            this.move_pin(&up_name, -1);
                            cx.notify();
                        });
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("pin-down-{down_name}")))
                    .small()
                    .label("下移")
                    .on_click(move |_, _, app| {
                        down_entity.update(app, |this, cx| {
                            this.move_pin(&down_name, 1);
                            cx.notify();
                        });
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("unpin-{unpin_name}")))
                    .small()
                    .label("取消钉住")
                    .on_click(move |_, _, app| {
                        unpin_entity.update(app, |this, cx| {
                            this.unpin_member(&unpin_name);
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

fn format_delay(delay: Option<u32>) -> String {
    match delay {
        None => String::new(),
        Some(0) => "超时".into(),
        Some(n) if n >= 10_000 => "失败".into(),
        Some(n) => format!("{n}ms"),
    }
}

fn render_group_card(
    entity: Entity<AppView>,
    theme: &Theme,
    group: &Group,
    count: usize,
    selected: bool,
    accent: Hsla,
    now: &str,
    members: &[(String, Option<u32>)],
    can_select: bool,
    delaying: bool,
    connected: bool,
) -> impl IntoElement {
    let id = group.id.clone();
    let del_id = group.id.clone();
    let group_name = group.name.clone();
    let muted = theme.muted;
    let muted_fg = theme.muted_foreground;
    let shown = members.iter().take(36).cloned().collect::<Vec<_>>();
    let extra = members.len().saturating_sub(shown.len());
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
                        .child(format!(
                            "{}  ·  {}  ·  {} 个节点",
                            group.name,
                            group.kind_label(),
                            count
                        )),
                )
                .child(
                    h_flex()
                        .gap_1()
                        .child({
                            let entity = entity.clone();
                            let name = group_name.clone();
                            Button::new(SharedString::from(format!("delay-group-{id}")))
                                .small()
                                .label(if delaying { "测延迟…" } else { "测延迟" })
                                .disabled(delaying || !connected)
                                .on_click(move |_, _, app| {
                                    app.stop_propagation();
                                    entity.update(app, |this, cx| {
                                        this.start_group_delay(&name, cx);
                                    });
                                })
                        })
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
                                        this.persist_and_apply(cx);
                                        cx.notify();
                                    });
                                    if close_modal {
                                        window.close_dialog(app);
                                    }
                                })
                        }),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted_fg)
                .child(format!("当前 {}  ·  {}", now, group.policy_label())),
        )
        .when(!shown.is_empty(), |this| {
            this.child(
                h_flex().w_full().flex_wrap().gap_1().children(
                    shown.into_iter().map(|(name, delay)| {
                        let delay_text = format_delay(delay);
                        let label = if delay_text.is_empty() {
                            name.clone()
                        } else {
                            format!("{name}  {delay_text}")
                        };
                        let is_now = name == now;
                        let entity = entity.clone();
                        let group_id = id.clone();
                        let mut button = Button::new(SharedString::from(format!(
                            "pick-{id}-{name}"
                        )))
                        .small()
                        .label(label);
                        if is_now {
                            button = button.primary();
                        }
                        if can_select {
                            button = button.on_click(move |_, _, app| {
                                app.stop_propagation();
                                entity.update(app, |this, cx| {
                                    this.select_group_member(&group_id, &name, cx);
                                });
                            });
                        }
                        button
                    }),
                ),
            )
        })
        .when(extra > 0, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child(format!("其余 {extra} 个在编辑窗查看")),
            )
        })
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
                                    this.move_selected_rule(&up_id, -1, cx);
                                    cx.notify();
                                });
                            }),
                    )
                    .item(
                        PopupMenuItem::new("下移")
                            .disabled(!can_down)
                            .on_click(move |_, _, app| {
                                down_entity.update(app, |this, cx| {
                                    this.move_selected_rule(&down_id, 1, cx);
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
                                    this.set_selected_rule_via(&id, &value, cx);
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
