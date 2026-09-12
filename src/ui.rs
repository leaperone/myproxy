use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::component::button::{
    Button, ButtonGroup, ButtonVariants as _, Toggle, ToggleGroup, ToggleVariants as _,
};
use gpui_kit::component::dialog::{Cancel, Confirm, DialogButtonProps, DialogFooter};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu, PopupMenu, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    h_flex, v_flex, ActiveTheme, Disableable, Icon, IconName, Root, Selectable, Sizable, StyledExt,
    Theme, TitleBar, WindowExt,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::KeyDownEvent;
use gpui_kit::*;
use myproxy::catalog::{self, Catalog};
use myproxy::controller::{
    self, ConnectionColumn, ConnectionFilters, LiveGroup, LiveNeed, TrafficSnapshot, TrafficTotals,
};
use myproxy::log;
use myproxy::network_extension::{self, Phase, RuntimeStatus};
use myproxy::setup::{self, SetupNext};
use myproxy::strategy::{
    join_list, parse_list, Group, InboundMode, Matcher, RoutingProfile, RuleSet, Strategy,
    GLOBAL_GROUP,
};
use myproxy::supervisor::{CoreHealth, OperationState, RuntimeIdentity, Supervisor};
use myproxy::theme_ext;
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
    member_query: Entity<InputState>,
    member_limit: usize,
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
        let member_query = cx.new(|cx| InputState::new(window, cx).placeholder("搜索预览节点…"));
        cx.observe(&member_query, |this, _, cx| {
            this.member_limit = 80;
            cx.notify();
        })
        .detach();
        Self {
            member_query,
            member_limit: 80,
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

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut next = self.draft(cx);
        if next.name.is_empty() {
            self.notice = "需要组名。".into();
            self.name.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return false;
        }
        let edit_id = self.edit_id.clone();
        let result = self.parent.update(cx, |parent, cx| {
            let previous = parent.strategy.clone();
            if let Some(id) = &edit_id {
                parent
                    .strategy
                    .update_group(id, next)
                    .map_err(|err| format!("保存失败：{err}"))?;
            } else {
                next.id = uuid::Uuid::new_v4().to_string();
                parent.strategy.add_group(next);
            }
            if parent.persist_and_apply(cx) {
                parent.group_modal_open = false;
                parent.group_edit_id = None;
                parent.status = if edit_id.is_some() {
                    "节点组已保存，正在应用…".into()
                } else {
                    "节点组已添加，正在应用…".into()
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
        let contains = parse_list(&self.contains.read(cx).value());
        let excludes = parse_list(&self.excludes.read(cx).value());
        let sources = parse_list(&self.sources.read(cx).value());
        let muted_fg = theme.muted_foreground;
        let border = theme.border;
        let radius = theme.radius;
        let group_box = theme.group_box;
        let subscriptions = parent.strategy.subscriptions.clone();
        let query = self.member_query.read(cx).value().trim().to_lowercase();
        let preview: Vec<_> = members
            .iter()
            .enumerate()
            .filter(|(_, name)| name.to_lowercase().contains(&query))
            .map(|(ix, name)| {
                (
                    name.clone(),
                    false,
                    (self.kind == "fallback").then_some(ix + 1),
                )
            })
            .chain(
                self.blocked
                    .iter()
                    .filter(|name| !members.contains(name) && name.to_lowercase().contains(&query))
                    .map(|name| (name.clone(), true, None)),
            )
            .collect();

        v_flex()
            .gap_3()
            .when(!self.notice.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme_ext::warning_text(&theme))
                        .child(self.notice.clone()),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(v_flex().gap_1().w(px(160.)).child(div().text_xs().child("组名")).child(Input::new(&self.name)))
                    .child({
                        let entity = entity.clone();
                        let mut group = ButtonGroup::new("group-mode").compact().outline().small();
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
                        let mut group = ButtonGroup::new("group-kind").compact().outline().small();
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
                    .child(v_flex().gap_1().flex_1().min_w_0().child(div().text_xs().child("名称包含")).child(Input::new(&self.contains)))
                    .children(REGION_PRESETS.iter().map(|(label, tokens)| {
                        let entity = entity.clone();
                        let tokens: Vec<String> = tokens.iter().map(|t| (*t).to_string()).collect();
                        Button::new(SharedString::from(format!("region-{label}")))
                            .small()
                            .label(*label)
                            .on_click(move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    let refs: Vec<&str> =
                                        tokens.iter().map(String::as_str).collect();
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
            .child(v_flex().gap_1().child(div().text_xs().child("名称不含（仅自动匹配）")).child(Input::new(&self.excludes)))
            .when(!excludes.is_empty(), |this| {
                this.child(chip_row(
                    entity.clone(),
                    "excludes",
                    ChipField::Excludes,
                    &excludes,
                ))
            })
            .child(div().text_xs().text_color(muted_fg).child(format!(
                "预览 · {} 个节点 · {} · {}",
                members.len(),
                draft.kind_setting_label(),
                draft.policy_label()
            )))
            .when(!draft.all_nodes && draft.name_contains.is_empty(), |this| {
                this.child(div().text_xs().text_color(muted_fg).child("仅选择来源不会自动加入节点；请添加名称条件或钉住节点。空组保持不可用，不会直连。"))
            })
            .child(v_flex().gap_1().child(div().text_xs().child("搜索预览成员（含排除项）")).child(Input::new(&self.member_query)))
            .child(
                v_flex().id("group-preview").max_h(px(240.)).overflow_y_scroll()
                    .rounded(radius).border_1().border_color(border).bg(group_box)
                    .when(preview.is_empty(), |this| this.child(div().p_3().text_xs().text_color(muted_fg).child(
                        if parent.catalog.nodes.is_empty() { "目录为空，请到订阅页刷新。" }
                        else if !query.is_empty() { "没有匹配搜索的成员。" }
                        else { "没有成员。请添加名称条件或钉住节点。" })))
                    .children(preview.iter().take(self.member_limit).map(|(name, blocked, rank)| {
                        render_member_row(entity.clone(), &theme, name, self.include.contains(name), *blocked, *rank)
                    }))
                    .when(preview.len() > self.member_limit, |this| {
                        let entity = entity.clone();
                        this.child(Button::new("group-preview-more").small()
                            .label(format!("继续显示（已显示 {} / {}）", self.member_limit, preview.len()))
                            .on_click(move |_, _, app| { entity.update(app, |this, cx| { this.member_limit += 80; cx.notify(); }); }))
                    }),
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
        let match_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(draft_kind.placeholder()));
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
        let parts = myproxy::strategy::parse_matcher_list(&raw);
        if parts.is_empty() {
            self.notice = format!("填写{}。", self.draft_kind.placeholder());
            cx.notify();
            return;
        }
        let matchers: Vec<_> = parts
            .into_iter()
            .map(|part| self.draft_kind.into_matcher(part))
            .collect();
        if let Some(error) = matchers.iter().find_map(|matcher| matcher.validate().err()) {
            self.notice = format!("匹配值无效：{error}");
            self.match_input.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        for matcher in matchers {
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

    fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.match_input.read(cx).value().trim().is_empty() {
            self.add_matchers(window, cx);
            if !self.match_input.read(cx).value().trim().is_empty() {
                return false;
            }
        }
        let mut next = self.draft(cx);
        if next.name.is_empty() {
            self.notice = "需要项目名。".into();
            self.name.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return false;
        }
        if next.matchers.is_empty() {
            self.notice = "至少加一条匹配。".into();
            cx.notify();
            return false;
        }
        if next.via.is_empty() {
            self.notice = "选走向：直连、拒绝、节点组、GFWList，或一个节点。".into();
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
            } else if parent
                .strategy
                .rule_sets
                .iter()
                .any(|set| set.name.eq_ignore_ascii_case(&next.name))
            {
                return Err("规则名称已存在，请编辑已有规则。".into());
            } else {
                next.id = uuid::Uuid::new_v4().to_string();
                parent.strategy.add_rule_set(next);
            }
            if parent.persist_and_apply(cx) {
                parent.rule_modal_open = false;
                parent.rule_edit_id = None;
                parent.status = if edit_id.is_some() {
                    "规则已保存，正在应用…".into()
                } else {
                    "规则已添加，正在应用…".into()
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
                        .text_color(theme_ext::warning_text(&theme))
                        .child(self.notice.clone()),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(v_flex().flex_1().gap_1().child(div().text_xs().child("规则名称")).child(Input::new(&self.name)))
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
                        let mut group = ButtonGroup::new("rule-kind").compact().outline().small();
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
                    .child(v_flex().flex_1().gap_1().child(div().text_xs().child("匹配值")).child(Input::new(&self.match_input)))
                    .child({
                        let entity = entity.clone();
                        Button::new("add-matcher").small().label("加入").on_click(
                            move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    this.add_matchers(window, cx);
                                });
                            },
                        )
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("逗号分隔批量加入；同一规则内任一匹配命中即生效。gfw: 由 mihomo 评 GFWList（命中走该组，未命中直连）；系统接管只把进程送到对应入口。"),
            )
            .when(self.matchers.is_empty(), |this| {
                this.child(div().text_xs().text_color(muted_fg).child("还没有匹配项。"))
            })
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .children(self.matchers.iter().enumerate().map(|(index, matcher)| {
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
                    })),
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
    if let Some(group) = myproxy::gfw::gfw_group(via) {
        return format!("GFWList → {}", via_label(group));
    }
    match via.to_ascii_lowercase().as_str() {
        "direct" => "直连".into(),
        "reject" => "拒绝".into(),
        _ => via.strip_prefix("node:").unwrap_or(via).to_string(),
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
        if out
            .iter()
            .any(|c| c.value.eq_ignore_ascii_case(&group.name))
        {
            continue;
        }
        out.push(ViaChoice {
            value: group.name.clone(),
            label: group.name.clone(),
            section: 1,
        });
    }
    for group in &strategy.groups {
        let value = format!("gfw:{}", group.name);
        if out.iter().any(|c| c.value.eq_ignore_ascii_case(&value)) {
            continue;
        }
        out.push(ViaChoice {
            value,
            label: format!("GFWList → {}", group.name),
            section: 2,
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
            section: 3,
        });
    }
    if let Some(via) = extra.map(str::trim).filter(|s| !s.is_empty()) {
        if !out.iter().any(|c| c.value.eq_ignore_ascii_case(via)) {
            out.push(ViaChoice {
                value: via.to_string(),
                label: via_label(via),
                section: 3,
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

enum LivePageJob {
    Rows(RuntimeIdentity, u64, bool),
    Snapshot(RuntimeIdentity, u64, LiveNeed),
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
    sidebar_compact: bool,
    strategy: Strategy,
    saved: Strategy,
    applied: Strategy,
    catalog: Catalog,
    status: String,
    connected: bool,
    wanted: bool,
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
    global_query: Entity<InputState>,
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
    delaying: HashSet<String>,
    window_active: bool,
    log_generation: u64,
    connection_filters: ConnectionFilters,
    runtime: Option<RuntimeIdentity>,
    operation: OperationState,
    extension_status: RuntimeStatus,
    live_revision: u64,
    member_query: Entity<InputState>,
    member_limits: HashMap<String, usize>,
    global_limit: usize,
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
            let mut first = true;
            loop {
                if !first {
                    let wait_ms = this
                        .update(cx, |this, _| this.live_poll_ms())
                        .unwrap_or(2500);
                    cx.background_executor()
                        .timer(Duration::from_millis(wait_ms))
                        .await;
                }
                first = false;

                let health_strategy = this
                    .update(cx, |this, _| {
                        if this.is_busy() {
                            None
                        } else {
                            Some((this.supervisor.clone(), this.applied.clone(), this.live_revision))
                        }
                    })
                    .ok()
                    .flatten();
                if let Some((supervisor, strategy, revision)) = health_strategy {
                    let _health = cx
                        .background_executor()
                        .spawn(async move { supervisor.observe(&strategy) })
                        .await;
                    if this
                        .update(cx, |this, cx| {
                            if !this.is_busy() && this.live_revision == revision && this.apply_health(this.supervisor.last_health()) {
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }

                let live_job = this
                    .update(cx, |this, cx| {
                        if !this.connected {
                            if this.clear_live() {
                                cx.notify();
                            }
                            None
                        } else {
                            this.live_page_job()
                        }
                    })
                    .ok()
                    .flatten();
                match live_job {
                    Some(LivePageJob::Rows(identity, revision, join_activity)) => {
                        let result = cx
                            .background_executor()
                            .spawn(async move {
                                controller::fetch(identity.mixed_port, join_activity)
                                    .map_err(|err| err.to_string())
                            })
                            .await;
                        if this
                            .update(cx, |this, cx| {
                                if this.accepts_live(identity, revision) && this.apply_traffic(result) {
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Some(LivePageJob::Snapshot(identity, revision, need)) => {
                        let result = cx
                            .background_executor()
                            .spawn(async move {
                                controller::fetch_live(identity.mixed_port, need, false)
                                    .map_err(|err| err.to_string())
                            })
                            .await;
                        if this
                            .update(cx, |this, cx| {
                                if this.accepts_live(identity, revision) && this.apply_live_snapshot(result) {
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    None => {}
                }

                if this
                    .update(cx, |this, cx| {
                        let started = Instant::now();
                        let mut dirty = this.sync_runtime();
                        if !this.is_busy() && !this.wanted && this.connected {
                            this.connected = false;
                            this.clear_live();
                            dirty = true;
                        }
                        let editing = this.group_modal_open || this.rule_modal_open;
                        if !this.is_busy() {
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
                        if this.page == Page::Settings {
                            let stamp = log::stamp();
                            if stamp != this.log_generation {
                                this.log_generation = stamp;
                                dirty = true;
                            }
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
        let wanted = supervisor.wanted();
        let entity = cx.entity();
        let appearance_observer = window.observe_window_appearance(move |window, cx| {
            entity.update(cx, |this, cx| {
                if this.appearance == Appearance::System {
                    Theme::sync_system_appearance(Some(window), cx);
                    cx.notify();
                }
            });
        });
        let rule_query = cx.new(|cx| InputState::new(window, cx).placeholder("筛选规则…"));
        let global_query = cx.new(|cx| InputState::new(window, cx).placeholder("筛选 GLOBAL…"));
        let member_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("搜索所有组的节点…"));
        cx.observe(&rule_query, |_, _, cx| cx.notify()).detach();
        cx.observe(&global_query, |this, _, cx| {
            this.global_limit = 36;
            cx.notify();
        })
        .detach();
        cx.observe(&member_query, |this, _, cx| {
            this.member_limits.clear();
            cx.notify();
        })
        .detach();
        let applied = supervisor
            .applied_strategy()
            .unwrap_or_else(|| strategy.clone());
        let this = Self {
            page: initial_page(),
            sidebar_compact: false,
            status: if catalog.nodes.is_empty() {
                "策略已加载。先添加订阅并刷新，再连接。".into()
            } else {
                "策略已加载。在总览连接；改端口或过滤器后点「应用」。".into()
            },
            connected: false,
            wanted,
            busy: false,
            cli_installed: myproxy::cli_install::is_installed(),
            external_change_pending: false,
            strategy_stamp: initial_strategy_stamp,
            supervisor,
            url_input: cx.new(|cx| InputState::new(window, cx).placeholder("https://…/clash.yaml")),
            name_input: cx.new(|cx| InputState::new(window, cx).placeholder("订阅名")),
            group_modal_open: false,
            group_edit_id: None,
            rule_modal_open: false,
            rule_edit_id: None,
            rule_query,
            global_query,
            filter_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(strategy.exclude_filter.clone())
            }),
            port_input: cx.new(|cx| {
                InputState::new(window, cx).default_value(strategy.mixed_port.to_string())
            }),
            strategy: strategy.clone(),
            saved: strategy.clone(),
            applied,
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
            delaying: HashSet::new(),
            window_active: true,
            log_generation: log::stamp(),
            pending_port_input: None,
            pending_filter_input: None,
            connection_filters: ConnectionFilters::default(),
            runtime: None,
            operation: OperationState::Idle,
            extension_status: network_extension::status(),
            live_revision: 0,
            member_query,
            member_limits: HashMap::new(),
            global_limit: 36,
        };
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

    fn is_busy(&self) -> bool {
        self.busy || self.supervisor.is_busy()
    }

    fn operation_label(&self) -> &str {
        match self.supervisor.operation_state() {
            OperationState::Connecting => "正在连接…",
            OperationState::Applying => "正在应用…",
            OperationState::Disconnecting => "正在断开…",
            _ if self.busy => &self.status,
            _ => self.connected_label(),
        }
    }

    fn mixed_endpoint(&self) -> String {
        self.runtime
            .map(|runtime| format!("127.0.0.1:{}", runtime.mixed_port))
            .unwrap_or_else(|| "未监听".into())
    }

    fn sync_runtime(&mut self) -> bool {
        let runtime = self.supervisor.runtime_identity();
        let operation = self.supervisor.operation_state();
        let extension_status = network_extension::status();
        let changed = self.runtime != runtime
            || self.operation != operation
            || self.extension_status != extension_status;
        if self.runtime != runtime {
            self.live_revision = self.live_revision.wrapping_add(1);
            self.clear_live();
            if runtime.is_some() {
                if let Some(applied) = self.supervisor.applied_strategy() {
                    self.applied = applied;
                }
            }
        }
        self.runtime = runtime;
        self.operation = operation;
        self.extension_status = extension_status;
        if self.strategy.system_extension {
            match self.extension_status.phase {
                Phase::WaitingApproval => {
                    self.status =
                        "系统接管等待授权。请在系统设置 › 通用 › 登录项与扩展中允许 myproxy。"
                            .into();
                }
                Phase::RequiresReboot => {
                    self.status = "系统接管需要重启后才能生效。".into();
                }
                _ => {}
            }
        }
        changed
    }

    fn accepts_live(&self, identity: RuntimeIdentity, revision: u64) -> bool {
        self.connected
            && !self.is_busy()
            && self.live_revision == revision
            && self.supervisor.runtime_identity() == Some(identity)
    }

    fn persist(&mut self) -> bool {
        self.persist_with_override(false)
    }

    fn persist_with_override(&mut self, overwrite_external: bool) -> bool {
        if self.is_busy() {
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
            self.status =
                "检测到外部策略变更。点击「覆盖并应用」保留本地配置，或重新打开窗口放弃本地修改。"
                    .into();
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
        if self.is_busy() {
            self.status = "正在处理上一项操作。".into();
            return false;
        }
        if !self.persist() {
            return false;
        }
        self.start_apply(cx)
    }

    fn start_apply(&mut self, cx: &mut Context<Self>) -> bool {
        self.start_apply_with_refresh(false, cx)
    }

    fn start_apply_with_refresh(&mut self, refresh: bool, cx: &mut Context<Self>) -> bool {
        if self.is_busy() {
            self.status = "正在处理上一项操作。".into();
            return false;
        }
        let strategy = self.strategy.clone();
        let refresh = refresh || !self.catalog.matches_strategy(&strategy);
        let supervisor = self.supervisor.clone();
        self.busy = true;
        self.live_revision = self.live_revision.wrapping_add(1);
        self.status = if refresh {
            "正在刷新订阅并应用策略…"
        } else {
            "正在应用策略…"
        }
        .into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let apply_strategy = strategy.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    (if refresh {
                        supervisor.apply(&apply_strategy)
                    } else {
                        supervisor.apply_cached(&apply_strategy)
                    })
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
                        this.sync_runtime();
                        this.status = format!(
                            "策略已应用 · {} 个节点 · Mixed {}。",
                            this.catalog.nodes.len(),
                            this.mixed_endpoint()
                        );
                        let failed = this.catalog.fetch_failure_count();
                        if failed > 0 {
                            this.status.push_str(&format!(" {failed} 个订阅拉取失败。"));
                        }
                        let warnings = this.catalog.refresh_warnings();
                        if !warnings.is_empty() {
                            this.status.push_str(&format!(" {}", warnings.join("；")));
                        }
                        if this.connected {
                            this.refresh_live(cx);
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
        let mut applied = self.applied.clone();
        // An empty saved selection lets the core choose its default member.
        // Only explicit selections must equal the observed runtime selection.
        if self.strategy.global_selected.is_empty() {
            applied.global_selected.clear();
        }
        for (intent, runtime) in self.strategy.groups.iter().zip(&mut applied.groups) {
            if intent.selected.is_empty() {
                runtime.selected.clear();
            }
        }
        self.strategy != applied
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

    fn apply_health(&mut self, health: CoreHealth) -> bool {
        let mut dirty = self.sync_runtime();
        if self.wanted != health.wanted {
            self.wanted = health.wanted;
            dirty = true;
        }
        let became_ready = health.ready && !self.connected;
        if self.connected != health.ready {
            self.connected = health.ready;
            if !health.ready {
                self.clear_live();
            }
            dirty = true;
        }
        if let Some(note) = health.note {
            if self.status != note {
                self.status = note;
                dirty = true;
            }
        } else if became_ready {
            self.status = format!("Mixed 已就绪 · {}（HTTP + SOCKS5）", self.mixed_endpoint());
            dirty = true;
        }
        dirty
    }

    fn connected_label(&self) -> &'static str {
        if self.connected {
            "已连接"
        } else if self.wanted {
            "核心异常"
        } else {
            "未连接"
        }
    }

    fn clear_live(&mut self) -> bool {
        let dirty = self.clear_traffic()
            || !self.proxy_groups.is_empty()
            || self.proxy_error.is_some()
            || !self.delaying.is_empty();
        self.proxy_groups.clear();
        self.proxy_error = None;
        self.delaying.clear();
        dirty
    }

    fn live_poll_ms(&self) -> u64 {
        if !self.window_active {
            return 4000;
        }
        match self.page {
            Page::Connections => 500,
            Page::Overview | Page::Groups => 1500,
            Page::Settings if self.strategy.uses_global() => 1500,
            _ => 2500,
        }
    }

    fn live_page_job(&mut self) -> Option<LivePageJob> {
        if !self.window_active || !self.connected || self.is_busy() {
            return None;
        }
        let runtime = self.supervisor.runtime_identity()?;
        self.live_revision = self.live_revision.wrapping_add(1);
        match self.page {
            Page::Connections => Some(LivePageJob::Rows(
                runtime,
                self.live_revision,
                self.strategy.system_extension,
            )),
            Page::Overview | Page::Groups => Some(LivePageJob::Snapshot(
                runtime,
                self.live_revision,
                LiveNeed::Totals,
            )),
            Page::Settings if self.strategy.uses_global() => Some(LivePageJob::Snapshot(
                runtime,
                self.live_revision,
                LiveNeed::Totals,
            )),
            _ => None,
        }
    }

    fn apply_live_snapshot(&mut self, result: Result<controller::LiveSnapshot, String>) -> bool {
        match result {
            Ok(snap) => {
                let traffic_changed = if snap.fetched_rows {
                    self.apply_traffic(Ok(snap.traffic))
                } else {
                    self.apply_totals(Ok(TrafficTotals {
                        upload_total: snap.traffic.upload_total,
                        download_total: snap.traffic.download_total,
                        connection_count: snap.traffic.connection_count,
                    }))
                };
                let proxies_changed = self.apply_proxies(Ok(snap.groups));
                traffic_changed || proxies_changed
            }
            Err(err) => {
                let traffic_changed = self.apply_totals(Err(err.clone()));
                let proxies_changed = self.apply_proxies(Err(err));
                traffic_changed || proxies_changed
            }
        }
    }

    fn apply_proxies(&mut self, result: Result<Vec<LiveGroup>, String>) -> bool {
        match result {
            Ok(groups) => {
                let changed = self.proxy_groups != groups || self.proxy_error.is_some();
                self.proxy_groups = groups;
                self.proxy_error = None;
                changed
            }
            Err(err) => {
                log::debug("ui", format!("proxies poll failed: {err}"));
                let msg = "暂时读不到节点组状态，当前显示为上次读取结果。";
                let changed = self.proxy_error.as_deref() != Some(msg);
                self.proxy_error = Some(msg.into());
                changed
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

    fn global_now(&self) -> String {
        if self.connected {
            return self.live_now(GLOBAL_GROUP).unwrap_or_default().to_string();
        }
        self.strategy.global_selected.trim().to_string()
    }

    fn global_members(&self, cx: &Context<Self>) -> Vec<(String, Option<u32>)> {
        let query = self.global_query.read(cx).value().trim().to_lowercase();
        let now = self.global_now();
        let mut names: Vec<String> = if let Some(live) = self
            .proxy_groups
            .iter()
            .find(|group| group.name == GLOBAL_GROUP)
        {
            live.members
                .iter()
                .map(|member| member.name.clone())
                .collect()
        } else {
            let mut names = vec!["DIRECT".into(), "REJECT".into()];
            for group in &self.strategy.groups {
                if !names.iter().any(|name| name == &group.name) {
                    names.push(group.name.clone());
                }
            }
            for node in &self.catalog.nodes {
                if !names.iter().any(|name| name == &node.name) {
                    names.push(node.name.clone());
                }
            }
            names
        };
        if !query.is_empty() {
            names.retain(|name| name.to_lowercase().contains(&query));
        }
        names.sort_by(|a, b| {
            let rank = |name: &str| {
                if name == now {
                    0
                } else if name.eq_ignore_ascii_case("DIRECT") {
                    1
                } else if name.eq_ignore_ascii_case("REJECT") {
                    2
                } else if self.strategy.groups.iter().any(|group| group.name == name) {
                    3
                } else {
                    4
                }
            };
            rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
        });
        names
            .into_iter()
            .map(|name| {
                let delay = self.member_delay(&name);
                (name, delay)
            })
            .collect()
    }

    fn overview_proxy_label(&self) -> String {
        if !self.wanted {
            return "未连接".into();
        }
        if !self.connected {
            return if self.proxy_error.is_some() {
                "读不到".into()
            } else {
                "异常".into()
            };
        }
        self.live_now("PROXY")
            .or_else(|| {
                self.proxy_groups
                    .iter()
                    .find(|group| group.name.eq_ignore_ascii_case("default"))
                    .map(|group| group.now.as_str())
                    .filter(|now| !now.is_empty())
            })
            .map(str::to_string)
            .unwrap_or_else(|| {
                if self.proxy_error.is_some() {
                    "读不到".into()
                } else {
                    "—".into()
                }
            })
    }

    fn refresh_live(&mut self, cx: &mut Context<Self>) {
        if !self.connected {
            self.clear_live();
            return;
        }
        let Some(job) = self.live_page_job() else {
            return;
        };
        cx.spawn(async move |this, cx| match job {
            LivePageJob::Rows(identity, revision, join_activity) => {
                let traffic = cx
                    .background_executor()
                    .spawn(async move {
                        controller::fetch(identity.mixed_port, join_activity)
                            .map_err(|err| err.to_string())
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if this.accepts_live(identity, revision) && this.apply_traffic(traffic) {
                        cx.notify();
                    }
                })
                .ok();
            }
            LivePageJob::Snapshot(identity, revision, need) => {
                let snap = cx
                    .background_executor()
                    .spawn(async move {
                        controller::fetch_live(identity.mixed_port, need, false)
                            .map_err(|err| err.to_string())
                    })
                    .await;
                this.update(cx, |this, cx| {
                    if this.accepts_live(identity, revision) && this.apply_live_snapshot(snap) {
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    fn note_totals(&mut self, upload_total: u64, download_total: u64) -> bool {
        let now = Instant::now();
        let mut rate_changed = false;
        if let Some((prev_at, prev_up, prev_down)) = self.traffic_prev {
            let dt = now.duration_since(prev_at).as_secs_f64();
            if dt >= 0.2 {
                let up = ((upload_total.saturating_sub(prev_up)) as f64 / dt) as u64;
                let down = ((download_total.saturating_sub(prev_down)) as f64 / dt) as u64;
                rate_changed =
                    self.traffic_up != up || self.traffic_down != down || !self.traffic_has_rate;
                self.traffic_up = up;
                self.traffic_down = down;
                self.traffic_has_rate = true;
                self.traffic_prev = Some((now, upload_total, download_total));
            }
        } else {
            self.traffic_prev = Some((now, upload_total, download_total));
        }
        rate_changed
    }

    fn apply_traffic(&mut self, result: Result<TrafficSnapshot, String>) -> bool {
        match result {
            Ok(snap) => {
                let rate_changed = self.note_totals(snap.upload_total, snap.download_total);
                let changed = self.traffic != snap || self.traffic_error.is_some() || rate_changed;
                self.traffic = snap;
                self.traffic_error = None;
                changed
            }
            Err(err) => {
                log::debug("ui", format!("connections poll failed: {err}"));
                let msg = "暂时读不到核心连接，列表为上次读取结果。";
                let changed = self.traffic_error.as_deref() != Some(msg);
                self.traffic_error = Some(msg.into());
                self.traffic_has_rate = false;
                self.traffic_prev = None;
                changed
            }
        }
    }

    fn apply_totals(&mut self, result: Result<TrafficTotals, String>) -> bool {
        match result {
            Ok(totals) => {
                let rate_changed = self.note_totals(totals.upload_total, totals.download_total);
                let changed = self.traffic.upload_total != totals.upload_total
                    || self.traffic.download_total != totals.download_total
                    || self.traffic.connection_count != totals.connection_count
                    || self.traffic_error.is_some()
                    || rate_changed;
                self.traffic.upload_total = totals.upload_total;
                self.traffic.download_total = totals.download_total;
                self.traffic.connection_count = totals.connection_count;
                self.traffic_error = None;
                changed
            }
            Err(err) => {
                log::debug("ui", format!("connections poll failed: {err}"));
                let msg = "暂时读不到核心连接，列表为上次读取结果。";
                let changed = self.traffic_error.as_deref() != Some(msg);
                self.traffic_error = Some(msg.into());
                self.traffic_has_rate = false;
                self.traffic_prev = None;
                changed
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
                this.live_revision = this.live_revision.wrapping_add(1);
                this.traffic_has_rate = false;
                this.traffic_prev = None;
                if this.connected
                    && matches!(page, Page::Connections | Page::Overview | Page::Groups)
                {
                    this.refresh_live(cx);
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
    ) -> Button {
        let compact = self.sidebar_compact;
        Button::new(SharedString::from(format!("nav-{label}")))
            .ghost()
            .w_full()
            .selected(self.page == page)
            .accessibility_label(label)
            .tooltip(label)
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_2()
                    .when(compact, |this| this.justify_center())
                    .when(!compact, |this| this.justify_start())
                    .child(Icon::new(icon).size_4().flex_shrink_0())
                    .when(!compact, |this| {
                        this.child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .child(label),
                        )
                    }),
            )
            .on_click(self.select_page(cx, page))
    }

    fn on_apply(
        &self,
        cx: &mut Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity();
        move |_, _, app| {
            entity.update(app, |this, cx| {
                if this.is_busy() {
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
        if self.is_busy() {
            self.status = "正在处理上一项操作。".into();
            cx.notify();
            return;
        }
        let connect = !self.supervisor.wanted();
        if connect && !self.persist() {
            cx.notify();
            return;
        }
        let strategy = self.strategy.clone();
        let supervisor = self.supervisor.clone();
        self.busy = true;
        self.live_revision = self.live_revision.wrapping_add(1);
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
                        this.wanted = true;
                        this.catalog = catalog;
                        if this.strategy == strategy {
                            this.mark_applied();
                        }
                        this.apply_health(this.supervisor.last_health());
                        if this.connected {
                            this.status = format!(
                                "Mixed 已就绪 · {}（HTTP + SOCKS5）",
                                this.mixed_endpoint()
                            );
                            this.refresh_live(cx);
                        } else if this.status.starts_with("正在连接") {
                            this.status = "核心已启动，正在确认是否可用…".into();
                        }
                    }
                    Ok(None) => {
                        this.wanted = false;
                        this.connected = false;
                        this.clear_live();
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
        let existing =
            id.and_then(|id| self.strategy.rule_sets.iter().find(|s| s.id == id).cloned());
        if id.is_some() && existing.is_none() {
            self.status = "找不到这条规则。".into();
            return;
        }
        self.present_rule_dialog(existing, window, cx);
    }

    fn open_rule_dialog_for_process(
        &mut self,
        process: &str,
        matcher: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let matcher = matcher.trim();
        if matcher.is_empty() || matcher == "—" {
            self.status = "这条连接没有可用的进程名。".into();
            return;
        }
        if let Some(existing) = self.strategy.rule_sets.iter().find(|set| {
            set.matchers.iter().any(|item| {
                item.kind == "app" && item.value.eq_ignore_ascii_case(matcher)
            })
        }) {
            self.open_rule_dialog(Some(&existing.id.clone()), window, cx);
            return;
        }
        self.present_rule_dialog(
            Some(RuleSet {
                id: String::new(),
                name: process.trim().to_string(),
                via: default_via(&self.strategy),
                matchers: vec![Matcher::app(matcher.to_string())],
            }),
            window,
            cx,
        );
    }

    fn set_connection_process_via(
        &mut self,
        process: &str,
        matcher: &str,
        via: &str,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let matcher = matcher.trim();
        if matcher.is_empty() || matcher == "—" {
            self.status = "这条连接没有可用的进程名。".into();
            return;
        }
        if let Some(set) = self.strategy.rule_sets.iter_mut().find(|set| {
            set.matchers.iter().any(|item| {
                item.kind == "app" && item.value.eq_ignore_ascii_case(matcher)
            })
        }) {
            set.via = via.to_string();
        } else {
            self.strategy.add_rule_set(RuleSet {
                id: uuid::Uuid::new_v4().to_string(),
                name: process.trim().to_string(),
                via: via.to_string(),
                matchers: vec![Matcher::app(matcher.to_string())],
            });
        }
        self.persist_and_apply(cx);
    }

    fn present_rule_dialog(
        &mut self,
        existing: Option<RuleSet>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.close_all_dialogs(cx);
        self.rule_modal_open = true;
        self.rule_edit_id = existing
            .as_ref()
            .and_then(|set| (!set.id.is_empty()).then(|| set.id.clone()));
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
        let editor =
            cx.new(|cx| RuleSetEditor::new(parent.clone(), existing, fallback_via, window, cx));
        let editing = self.rule_edit_id.is_some();
        let initial_focus = editor.read(cx).name.read(cx).focus_handle(cx);
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
                                    window
                                        .dispatch_action(Box::new(Confirm { secondary: false }), cx)
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
        initial_focus.focus(window, cx);
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
        let initial_focus = editor.read(cx).name.read(cx).focus_handle(cx);
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
                                    window
                                        .dispatch_action(Box::new(Confirm { secondary: false }), cx)
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
        initial_focus.focus(window, cx);
    }

    fn set_selected_rule_via(&mut self, id: &str, via: &str, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if !self.strategy.set_rule_via(id, via.to_string()) {
            self.status = "改走向失败。".into();
            return;
        }
        self.persist_and_apply(cx);
    }

    fn move_selected_rule(&mut self, id: &str, delta: i32, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if !self.strategy.move_rule(id, delta) {
            return;
        }
        self.persist_and_apply(cx);
    }

    fn remove_selected_rule(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let previous = self.strategy.clone();
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
            self.status = "规则已删除，正在应用…".into();
        } else {
            self.strategy = previous;
        }
    }

    fn live_group(&self, group: &Group) -> Option<&LiveGroup> {
        self.proxy_groups
            .iter()
            .find(|live| live.name == group.name)
    }

    fn group_now<'a>(&'a self, group: &'a Group) -> &'a str {
        if self.connected {
            return match self.live_group(group) {
                Some(live) if live.members.is_empty() => "不可用",
                Some(live) if !live.now.is_empty() => &live.now,
                _ => "等待核心状态",
            };
        }
        if catalog::resolve_group_members(group, &self.catalog).is_empty() {
            "不可用"
        } else if group.selected.is_empty() {
            "—"
        } else {
            &group.selected
        }
    }

    fn group_member_names(&self, group: &Group) -> Vec<String> {
        if self.connected {
            return self
                .live_group(group)
                .map(|live| {
                    live.members
                        .iter()
                        .map(|member| member.name.clone())
                        .collect()
                })
                .unwrap_or_default();
        }
        catalog::resolve_group_members(group, &self.catalog)
    }

    fn member_delay(&self, name: &str) -> Option<u32> {
        self.proxy_groups
            .iter()
            .flat_map(|group| group.members.iter())
            .find(|member| member.name == name)
            .and_then(|member| member.delay)
    }

    fn select_global_member(&mut self, node: &str, cx: &mut Context<Self>) {
        self.select_member(GLOBAL_GROUP, node, cx);
    }

    fn select_group_member(&mut self, group_id: &str, node: &str, cx: &mut Context<Self>) {
        self.select_member(group_id, node, cx);
    }

    fn select_member(&mut self, group_id: &str, node: &str, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let previous = self.strategy.clone();
        let group_name = if group_id == GLOBAL_GROUP {
            self.strategy.set_global_selected(node.to_string());
            GLOBAL_GROUP.to_string()
        } else {
            if !self.strategy.set_group_selected(group_id, node.to_string()) {
                self.status = "只能在手动选择组里点选节点。".into();
                cx.notify();
                return;
            }
            self.strategy
                .groups
                .iter()
                .find(|group| group.id == group_id || group.name == group_id)
                .map(|group| group.name.clone())
                .unwrap_or_default()
        };
        if !self.persist() {
            self.strategy = previous;
            cx.notify();
            return;
        }
        let Some(identity) = self
            .supervisor
            .runtime_identity()
            .filter(|_| self.connected)
        else {
            self.status = "选择已保存，下次连接后生效。".into();
            cx.notify();
            return;
        };
        self.busy = true;
        self.live_revision = self.live_revision.wrapping_add(1);
        self.status = format!("正在将 {group_name} 切换到 {node}…");
        let node = node.to_string();
        let supervisor = self.supervisor.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let request_group = group_name.clone();
            let request_node = node.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    supervisor
                        .select_proxy(identity, &request_group, &request_node)
                        .map_err(|err| err.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                this.sync_runtime();
                match result {
                    Ok(()) => this.status = format!("{group_name} 已切换到 {node}。"),
                    Err(err) => this.status = format!("切换失败：{err}。选择已保存，尚未生效。"),
                }
                this.refresh_live(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn close_connections(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let Some(identity) = self
            .supervisor
            .runtime_identity()
            .filter(|_| self.connected)
        else {
            return;
        };
        self.busy = true;
        self.live_revision = self.live_revision.wrapping_add(1);
        self.status = if id.is_some() {
            "正在关闭连接…"
        } else {
            "正在关闭核心的全部连接（包含筛选外连接）…"
        }
        .into();
        let supervisor = self.supervisor.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let request_id = id.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    match request_id {
                        Some(id) => supervisor.close_one(identity, &id),
                        None => supervisor.close_all(identity),
                    }
                    .map_err(|err| err.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                if this.supervisor.runtime_identity() == Some(identity) {
                    match result {
                        Ok(()) => {
                            if let Some(id) = &id {
                                this.traffic.connections.retain(|conn| &conn.id != id);
                            } else {
                                this.traffic.connections.clear();
                            }
                            this.status = if id.is_some() {
                                "连接已关闭。"
                            } else {
                                "核心的全部连接已关闭。"
                            }
                            .into();
                        }
                        Err(err) => this.status = format!("关闭失败：{err}"),
                    }
                } else {
                    this.status = "运行配置已变化，请重新确认连接列表。".into();
                }
                this.refresh_live(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_group_delay(&mut self, group_name: &str, cx: &mut Context<Self>) {
        if !self.connected || self.is_busy() {
            self.status = "核心空闲且连接后再测延迟。".into();
            cx.notify();
            return;
        }
        if !self.delaying.insert(group_name.to_string()) {
            return;
        }
        self.status = format!("正在测 {group_name} 延迟…");
        let Some(identity) = self.supervisor.runtime_identity() else {
            return;
        };
        let name = group_name.to_string();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let probe = name.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    controller::test_group_delay(identity.mixed_port, &probe)
                        .map_err(|err| err.to_string())
                })
                .await;
            this.update(cx, |this, cx| {
                this.delaying.remove(&name);
                if this.supervisor.runtime_identity() != Some(identity) {
                    return;
                }
                match result {
                    Ok(_) => {
                        this.status = format!("{name} 探测完成，正在读取核心最新探测记录。");
                        this.refresh_live(cx);
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
        let active = window.is_window_active();
        if active && !self.window_active && self.connected {
            self.window_active = true;
            self.refresh_live(cx);
        } else {
            if !active && self.window_active {
                self.live_revision = self.live_revision.wrapping_add(1);
                self.traffic_has_rate = false;
                self.traffic_prev = None;
            }
            self.window_active = active;
        }
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
                            .key_context("shell")
                            .flex_1()
                            .overflow_hidden()
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                                if event.keystroke.key.eq_ignore_ascii_case("b")
                                    && event.keystroke.modifiers.platform
                                    && !event.keystroke.modifiers.control
                                    && !event.keystroke.modifiers.alt
                                    && !event.keystroke.modifiers.shift
                                {
                                    this.sidebar_compact = !this.sidebar_compact;
                                    cx.stop_propagation();
                                    cx.notify();
                                }
                            }))
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
        let busy = self.is_busy();
        let warn_live = !busy
            && (self.wanted && !connected
                || self.traffic_error.is_some()
                || self.proxy_error.is_some());
        let live_label = if busy {
            self.operation_label().to_string()
        } else if connected && self.traffic_has_rate {
            format!(
                "已连接 · ↑{} · ↓{}",
                controller::format_rate(self.traffic_up),
                controller::format_rate(self.traffic_down)
            )
        } else if connected && self.traffic_error.is_some() {
            "读不到核心状态".into()
        } else if connected {
            "已连接".into()
        } else if self.wanted {
            self.status
                .lines()
                .next()
                .filter(|line| !line.is_empty())
                .unwrap_or("核心异常")
                .to_string()
        } else {
            "未连接".into()
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
                        .child(div().text_sm().font_semibold().child("MyProxy"))
                        .children(
                            updates::build_badge().map(|label| pill(theme, label)),
                        ),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(status_dot(theme, connected && !warn_live))
                        .child(
                            div()
                                .text_xs()
                                .text_color(if warn_live {
                                    theme_ext::warning_text(theme)
                                } else {
                                    theme.muted_foreground
                                })
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
        v_flex()
            .id("nav")
            .flex_shrink_0()
            .h_full()
            .gap_2()
            .p_2()
            .w(px(if self.sidebar_compact { 56. } else { 216. }))
            .bg(theme.sidebar)
            .child(self.sidebar_header(cx, theme))
            .child(
                v_flex()
                    .id("nav-items")
                    .w_full()
                    .gap_1()
                    .child(self.nav_item(cx, Page::Overview, "总览", IconName::LayoutDashboard))
                    .child(self.nav_item(cx, Page::Connections, "连接", IconName::Network))
                    .child(self.nav_item(cx, Page::Subscriptions, "订阅", IconName::Inbox))
                    .child(self.nav_item(cx, Page::Groups, "节点组", IconName::Folder))
                    .child(self.nav_item(cx, Page::Rules, "规则", IconName::Map))
                    .child(self.nav_item(cx, Page::Settings, "设置", IconName::Settings)),
            )
    }

    fn sidebar_header(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let compact = self.sidebar_compact;
        let toggle = Button::new("toggle-sidebar")
            .ghost()
            .small()
            .icon(if compact {
                IconName::PanelLeftOpen
            } else {
                IconName::PanelLeft
            })
            .tooltip(if compact { "展开侧栏" } else { "收起侧栏" })
            .accessibility_label(if compact { "展开侧栏" } else { "收起侧栏" })
            .on_click({
                let entity = cx.entity();
                move |_, _, app| {
                    entity.update(app, |this, cx| {
                        this.sidebar_compact = !this.sidebar_compact;
                        cx.notify();
                    });
                }
            });
        h_flex()
            .id("nav-header")
            .w_full()
            .h_8()
            .items_center()
            .when(compact, |this| this.justify_center())
            .when(!compact, |this| {
                this.justify_between().pl_2().child(
                    div()
                        .text_xs()
                        .font_medium()
                        .text_color(theme.muted_foreground)
                        .child("导航"),
                )
            })
            .child(toggle)
    }

    fn page_view(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        v_flex()
            .id("page")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_hidden()
            .p_6()
            .gap_4()
            .bg(theme.background)
            .child(
                div()
                    .id("status")
                    .text_xs()
                    .text_color(
                        if self.status.contains("失败")
                            || self.status.contains("异常")
                            || self.status.contains("无响应")
                        {
                            theme_ext::warning_text(theme)
                        } else {
                            theme.muted_foreground
                        },
                    )
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

    fn setup_next(&self) -> SetupNext {
        setup::next_setup_action(
            self.strategy.subscriptions.len(),
            self.catalog.nodes.len(),
            self.connected,
        )
    }

    fn overview_connect_button(&self, cx: &mut Context<Self>, primary: bool) -> Button {
        let busy = self.is_busy();
        let mut connect = Button::new("hero-connect").large();
        connect = if busy {
            connect.label(self.operation_label().to_string())
        } else if self.wanted {
            connect.label("断开")
        } else if primary {
            connect.primary().label("连接")
        } else {
            connect.label("连接")
        };
        connect
            .h(px(48.))
            .px_8()
            .min_w(px(132.))
            .disabled(busy)
            .on_click(self.on_connect(cx))
    }

    fn overview_hero(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let next = self.setup_next();
        let entity = cx.entity();
        match next {
            SetupNext::AddSubscription => h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("hero-add-subscription")
                        .large()
                        .primary()
                        .label("添加订阅")
                        .h(px(48.))
                        .px_8()
                        .on_click(self.select_page(cx, Page::Subscriptions)),
                )
                .child(self.overview_connect_button(cx, false))
                .into_any_element(),
            SetupNext::RefreshCatalog => h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("hero-refresh-subscriptions")
                        .large()
                        .primary()
                        .label("刷新订阅并应用")
                        .h(px(48.))
                        .px_8()
                        .disabled(busy)
                        .on_click({
                            let entity = entity.clone();
                            move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    if this.persist() {
                                        this.start_apply_with_refresh(true, cx);
                                    }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(self.overview_connect_button(cx, false))
                .into_any_element(),
            SetupNext::Connect | SetupNext::Ready => {
                self.overview_connect_button(cx, true).into_any_element()
            }
        }
    }

    fn overview_setup_card(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let next = self.setup_next();
        let has_sub = !self.strategy.subscriptions.is_empty();
        let has_nodes = !self.catalog.nodes.is_empty();
        let muted = theme.muted_foreground;
        let inbound = format!(
            "规则只处理进入代理的流量。Mixed 把客户端代理设为 {}（HTTP + SOCKS5）。系统接管在设置里打开，并到 {} 允许 myproxy。TUN 与接管互斥，首次要管理员密码。",
            self.mixed_endpoint(),
            setup::login_items_path_label()
        );
        v_flex()
            .w_full()
            .p_4()
            .gap_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.group_box)
            .child(div().text_sm().font_semibold().child("配置"))
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if has_sub {
                        "1. 添加订阅 — 已完成"
                    } else {
                        "1. 添加订阅 — 下一步"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if has_nodes {
                        "2. 刷新得到节点 — 已完成"
                    } else {
                        "2. 刷新得到节点 — 有订阅后点「刷新订阅并应用」"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted)
                    .child(if self.connected {
                        "3. 连接核心 — 已连接"
                    } else {
                        "3. 连接核心 — 有节点后再连。空组出口不可用，不会直连。"
                    }),
            )
            .child(div().text_xs().text_color(muted).child(inbound))
            .when(next == SetupNext::AddSubscription, |this| {
                this.child(
                    Button::new("setup-add-subscription")
                        .small()
                        .primary()
                        .label("添加订阅")
                        .on_click(self.select_page(cx, Page::Subscriptions)),
                )
            })
    }

    fn overview(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let connected = self.connected;
        let wanted = self.wanted;
        let up = if connected && self.traffic_error.is_some() {
            "读不到".into()
        } else if connected && self.traffic_has_rate {
            controller::format_rate(self.traffic_up)
        } else {
            "—".into()
        };
        let down = if connected && self.traffic_error.is_some() {
            "读不到".into()
        } else if connected && self.traffic_has_rate {
            controller::format_rate(self.traffic_down)
        } else {
            "—".into()
        };
        let conns = if connected && self.traffic_error.is_some() {
            "读不到".into()
        } else if connected {
            self.traffic.connection_count.to_string()
        } else {
            "—".into()
        };
        let now = self.overview_proxy_label();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "总览",
                &self.inbound_modes_subtitle(),
            ))
            .child(self.overview_setup_card(cx, theme))
            .child(
                h_flex()
                    .w_full()
                    .p_5()
                    .gap_4()
                    .flex_wrap()
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
                                    .child(status_dot(theme, connected))
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_semibold()
                                            .child(self.connected_label()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child(if connected {
                                        format!("Mixed {} · 系统接管 {} · DNS {}{}", self.mixed_endpoint(),
                                            self.extension_status.phase_label(), self.extension_status.dns_label(),
                                            if self.applied.tun { " · TUN 按策略规则运行" } else { "" })
                                    } else if wanted {
                                        "核心尚未就绪，请查看操作状态。".into()
                                    } else {
                                        format!(
                                            "连接后 Mixed {} 可供显式代理客户端使用。系统接管和 TUN 在设置里分别打开；规则只处理进入这三条入站的流量。",
                                            self.mixed_endpoint()
                                        )
                                    }),
                            ),
                    )
                    .child(self.overview_hero(cx)),
            )
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(metric(theme, "上传", &up))
                    .child(metric(theme, "下载", &down))
                    .child(metric(theme, "连接数", &conns)),
            )
            .child(
                h_flex().gap_3().flex_wrap()
                    .child(metric(theme, "PROXY", &now))
                    .child(metric(theme, "GLOBAL", &global_selection_label(&self.global_now(), self.connected)))
                    .child(metric(
                        theme,
                        "系统接管",
                        self.extension_status.phase_label(),
                    ))
                    .child(metric(
                        theme,
                        "Mixed",
                        &self.mixed_endpoint(),
                    ))
                    .child(metric(
                        theme,
                        "节点",
                        &format!(
                            "保留 {} / 过滤 {} / 拉取失败 {}",
                            self.catalog.nodes.len(),
                            self.catalog.filter_excluded_count(),
                            self.catalog.fetch_failure_count()
                        ),
                    )),
            )
            .when(
                self.traffic_error.is_some() || self.proxy_error.is_some(),
                |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme_ext::warning_text(theme))
                            .child(
                                self.traffic_error
                                    .clone()
                                    .or_else(|| self.proxy_error.clone())
                                    .unwrap_or_default(),
                            ),
                    )
                },
            )
    }

    fn connections(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let connected = self.connected;
        let muted_fg = theme.muted_foreground;
        let up = if connected && self.traffic_error.is_some() {
            "读不到".into()
        } else if connected && self.traffic_has_rate {
            controller::format_rate(self.traffic_up)
        } else {
            "—".into()
        };
        let down = if connected && self.traffic_error.is_some() {
            "读不到".into()
        } else if connected && self.traffic_has_rate {
            controller::format_rate(self.traffic_down)
        } else {
            "—".into()
        };
        let count = if connected && self.traffic_error.is_some() {
            "读不到".into()
        } else if connected {
            self.traffic.connection_count.to_string()
        } else {
            "—".into()
        };
        let mixed = self.mixed_endpoint();
        let filtered =
            controller::filter_connections(&self.traffic.connections, &self.connection_filters);
        let filter_note = if !connected || self.traffic.connections.is_empty() {
            String::new()
        } else if filtered.len() == self.traffic.connections.len()
            && self.connection_filters.show_direct
            && !self.connection_filters.has_column_filter()
        {
            String::new()
        } else {
            format!(
                "当前筛选 {} / 已加载 {} 条",
                filtered.len(),
                self.traffic.connections.len()
            )
        };
        v_flex()
            .id("connections-page")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .gap_4()
            .child(page_title(
                theme,
                "连接",
                if self.strategy.system_extension {
                    "经过 Mihomo 的连接。系统接管中继后会尽量显示真实进程；扩展直接放行或拒绝的活动不在此表。"
                } else {
                    "经过 Mihomo 的连接。当前只看到主动指定 Mixed 的客户端。打开系统接管后，未填代理的应用也会出现。"
                },
            ))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .child(metric(
                        theme,
                        "状态",
                        self.connected_label(),
                    ))
                    .child(metric(theme, "Mixed 端口", &mixed))
                    .child(metric(theme, "上传", &up))
                    .child(metric(theme, "下载", &down))
                    .child(metric(theme, "连接数", &count)),
            )
            .when(
                self.traffic_error.is_some() || (connected && !self.traffic.connections.is_empty()),
                |this| {
                    this.child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(if self.traffic_error.is_some() {
                                        theme_ext::warning_text(theme)
                                    } else {
                                        muted_fg
                                    })
                                    .child(
                                        self.traffic_error
                                            .clone()
                                            .unwrap_or(filter_note),
                                    ),
                            )
                            .when(connected && !self.traffic.connections.is_empty(), |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child({
                                            let entity = entity.clone();
                                            Toggle::new("show-direct-connections")
                                                .small()
                                                .outline()
                                                .label("显示直连")
                                                .checked(self.connection_filters.show_direct)
                                                .on_click(move |checked, _, app| {
                                                    let show = *checked;
                                                    entity.update(app, |this, cx| {
                                                        this.connection_filters.show_direct = show;
                                                        cx.notify();
                                                    });
                                                })
                                        })
                                        .when(
                                            self.connection_filters.has_column_filter(),
                                            |this| {
                                                this.child({
                                                    let entity = entity.clone();
                                                    Button::new("clear-connection-filters")
                                                        .small()
                                                        .ghost()
                                                        .label("清除筛选")
                                                        .on_click(move |_, _, app| {
                                                            entity.update(app, |this, cx| {
                                                                this.connection_filters
                                                                    .clear_columns();
                                                                cx.notify();
                                                            });
                                                        })
                                                })
                                            },
                                        )
                                        .child({
                                            let entity = entity.clone();
                                            Button::new("close-all-connections")
                                                .small()
                                                .danger()
                                                .label("关闭核心全部连接")
                                                .disabled(self.is_busy())
                                                .tooltip("关闭 Mihomo 全部连接，包含当前筛选外的连接")
                                                .on_click(move |_, _, app| {
                                                    entity.update(app, |this, cx| this.close_connections(None, cx));
                                                })
                                        }),
                                )
                            }),
                    )
                },
            )
            .when(!connected && !self.wanted, |this| {
                this.child(empty_hint_action(
                    theme,
                    "核心未连接。到总览连接后，这里显示经过 Mihomo 的连接。",
                    Button::new("connections-go-overview")
                        .primary()
                        .label("去总览连接")
                        .on_click(self.select_page(cx, Page::Overview)),
                ))
            })
            .when(!connected && self.wanted, |this| {
                this.child(empty_hint(
                    theme,
                    "核心未就绪。恢复后显示经过 Mihomo 的连接。",
                ))
            })
            .when(
                connected && self.traffic.connections.is_empty() && self.traffic_error.is_none(),
                |this| {
                    this.child(empty_hint(
                        theme,
                        if self.strategy.system_extension {
                            "核心目前未记录连接。系统接管已打开时，被中继进 Mihomo 的应用会列在这里。"
                        } else {
                            "核心目前未记录连接。只有进入 Mixed 或被系统接管中继的流量会出现在这里。"
                        },
                    ))
                },
            )
            .when(
                connected && self.traffic.connections.is_empty() && self.traffic_error.is_some(),
                |this| {
                    this.child(empty_hint(
                        theme,
                        "核心在跑，但读不到连接列表。不是没有流量，是控制器没响应。",
                    ))
                },
            )
            .when(connected && !self.traffic.connections.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id("connection-list")
                        .flex_1()
                        .min_h_0()
                        .min_w_0()
                        .w_full()
                        .overflow_x_scroll()
                        .overflow_y_scroll()
                        .child(v_flex().min_w(px(920.)).w_full().gap_1()
                        .child(connection_header_row(
                            entity.clone(),
                            theme,
                            &self.traffic.connections,
                            &self.connection_filters,
                        ))
                        .when(filtered.is_empty(), |this| {
                            this.child(
                                div()
                                    .px_3()
                                    .py_2()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(if self.connection_filters.show_direct {
                                        "没有符合筛选的连接。"
                                    } else {
                                        "没有符合筛选的连接。打开「显示直连」可查看直连。"
                                    }),
                            )
                        })
                        .children(filtered.into_iter().map(|conn| {
                            render_connection_row(
                                entity.clone(),
                                theme,
                                conn,
                                via_choices(&self.strategy, &self.catalog, None),
                                self.is_busy(),
                            )
                        }))
                        .when(
                            self.traffic.connection_count > self.traffic.connections.len(),
                            |this| {
                                this.child(
                                    div().px_3().py_2().text_xs().text_color(muted_fg).child(
                                        format!(
                                            "当前仅加载流量最高的 {} 条，核心共 {} 条；筛选只作用于已加载列表。",
                                            self.traffic.connections.len(),
                                            self.traffic.connection_count
                                        ),
                                    ),
                                )
                            },
                        ),
                ))
            })
    }

    fn subscriptions(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "订阅",
                "添加或删除后需应用。规则和模式变更使用匹配的缓存；刷新按钮会重新获取订阅。",
            ))
            .child(
                Button::new("refresh-subscriptions")
                    .label("刷新订阅并应用")
                    .disabled(self.is_busy())
                    .on_click({
                        let entity = entity.clone();
                        move |_, _, app| {
                            entity.update(app, |this, cx| {
                                if this.persist() {
                                    this.start_apply_with_refresh(true, cx);
                                }
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_end()
                    .flex_wrap()
                    .child(
                        v_flex()
                            .gap_1()
                            .w(px(160.))
                            .child(div().text_xs().child("订阅名"))
                            .child(Input::new(&self.name_input)),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w(px(180.))
                            .child(div().text_xs().child("订阅 URL"))
                            .child(Input::new(&self.url_input)),
                    )
                    .child(
                        Button::new("add-sub")
                            .primary()
                            .label("添加")
                            .disabled(self.is_busy())
                            .on_click({
                                let entity = entity.clone();
                                move |_, window, app| {
                                    entity.update(app, |this, cx| {
                                        if this.is_busy() {
                                            return;
                                        }
                                        let name =
                                            this.name_input.read(cx).value().trim().to_string();
                                        let url =
                                            this.url_input.read(cx).value().trim().to_string();
                                        if name.is_empty() || url.is_empty() {
                                            this.status = "填写订阅名和订阅 URL。".into();
                                        } else {
                                            let previous = this.strategy.clone();
                                            this.strategy.add_subscription(name, url);
                                            if this.persist() {
                                                this.status =
                                                    "订阅已添加，尚未应用。请刷新订阅并应用。"
                                                        .into();
                                                this.name_input.update(cx, |input, cx| {
                                                    input.set_value("", window, cx)
                                                });
                                                this.url_input.update(cx, |input, cx| {
                                                    input.set_value("", window, cx)
                                                });
                                            } else {
                                                this.strategy = previous;
                                            }
                                        }
                                        cx.notify();
                                    });
                                }
                            }),
                    ),
            )
            .when(self.strategy.subscriptions.is_empty(), |this| {
                this.child(empty_hint(
                    theme,
                    "还没有订阅。在上方填写名称和 URL，然后点添加。",
                ))
            })
            .children(self.strategy.subscriptions.iter().map(|sub| {
                let entity = entity.clone();
                let id = sub.id.clone();
                let warning = self.catalog.subscription_warning(&sub.name);
                panel(
                    theme,
                    &sub.name,
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .gap_2()
                                .justify_between()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .font_family(theme.mono_font_family.clone())
                                        .text_color(theme.muted_foreground)
                                        .child(sub.url.clone()),
                                )
                                .child(
                                    Button::new(SharedString::from(format!("del-sub-{id}")))
                                        .small()
                                        .danger()
                                        .label("删除")
                                        .disabled(self.is_busy())
                                        .on_click(move |_, _, app| {
                                            entity.update(app, |this, cx| {
                                                if this.is_busy() {
                                                    return;
                                                }
                                                let previous = this.strategy.clone();
                                                this.strategy.remove_subscription(&id);
                                                if this.persist() {
                                                    this.status =
                                                        "订阅已删除，尚未应用。请应用配置。".into();
                                                } else {
                                                    this.strategy = previous;
                                                }
                                                cx.notify();
                                            });
                                        }),
                                ),
                        )
                        .when_some(warning, |this, warning| {
                            this.child(
                                div()
                                    .text_xs()
                                    .text_color(theme_ext::warning_text(theme))
                                    .child(warning),
                            )
                        }),
                )
            }))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "当前目录 {} 个节点，过滤排除 {}，拉取失败 {}。",
                        self.catalog.nodes.len(),
                        self.catalog.filter_excluded_count(),
                        self.catalog.fetch_failure_count()
                    )),
            )
    }

    fn groups(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let query = self.member_query.read(cx).value().trim().to_lowercase();
        v_flex()
            .gap_4()
            .child(page_title(
                theme,
                "节点组",
                "手动组可选节点，自动组展示核心当前成员；点击「编辑」调整条件，卡片边框仅表示编辑中。来源约束名称匹配，钉住优先，精确排除最终生效。",
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
            .child({
                let now = self.global_now();
                let members = self.global_members(cx);
                render_global_card(
                    entity.clone(),
                    theme,
                    &now,
                    &members,
                    self.delaying.contains(GLOBAL_GROUP),
                    self.connected,
                    self.strategy.uses_global(),
                    &self.global_query,
                    self.global_limit,
                    self.is_busy(),
                )
            })
            .child(v_flex().gap_1().child(div().text_xs().child("搜索节点组成员")).child(Input::new(&self.member_query)))
            .when(self.strategy.groups.is_empty(), |this| {
                this.child(empty_hint_action(
                    theme,
                    "还没有节点组。添加一个组，或导入带默认 PROXY 的配置。",
                    Button::new("groups-add-empty")
                        .primary()
                        .label("添加节点组")
                        .on_click({
                            let entity = entity.clone();
                            move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    this.open_group_dialog(None, window, cx);
                                    cx.notify();
                                });
                            }
                        }),
                ))
            })
            .when(self.connected && self.proxy_error.is_some(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme_ext::warning_text(theme))
                        .child(self.proxy_error.clone().unwrap_or_default()),
                )
            })
            .children(self.strategy.groups.iter().map(|group| {
                let member_names = self.group_member_names(group);
                let count = if self.connected {
                    self.live_group(group).map(|live| live.members.len())
                } else {
                    Some(member_names.len())
                };
                let selected = self.group_edit_id.as_deref() == Some(group.id.as_str());
                let now = self.group_now(group).to_string();
                let members: Vec<(String, Option<u32>)> = member_names
                    .into_iter()
                    .filter(|name| name.to_lowercase().contains(&query))
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
                    &now,
                    &members,
                    group.kind == "select",
                    self.delaying.contains(&group.name),
                    self.connected,
                    *self.member_limits.get(&group.id).unwrap_or(&36),
                    self.is_busy(),
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
        v_flex()
            .id("rules-page")
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .gap_4()
            .child(page_title(
                theme,
                "规则",
                "规则入口先绕过本机和私网，再按下表自上而下首条命中。同一规则内是任一条件命中；GFWList 预设独立装卸。",
            ))
            .child(self.routing_panel(cx, theme))
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
                this.child(empty_hint_action(
                    theme,
                    "还没有规则。添加进程或域名，再选走向。未命中的流量按本页分流预设走。",
                    Button::new("rules-add-empty")
                        .primary()
                        .label("添加规则")
                        .on_click({
                            let entity = entity.clone();
                            move |_, window, app| {
                                entity.update(app, |this, cx| {
                                    this.open_rule_dialog(None, window, cx);
                                    cx.notify();
                                });
                            }
                        }),
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
                &format!(
                    "系统接管让应用不用自己填代理。第一次请到 {} 允许 myproxy。Mixed 给显式客户端；TUN 与接管互斥。",
                    setup::login_items_path_label()
                ),
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
                            .child(format!(
                                "把客户端 HTTP 或 SOCKS5 代理设为 {}。系统接管按捕获规则转入核心；原生放行和拒绝不进连接列表。",
                                self.mixed_endpoint()
                            )),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .flex_wrap()
                            .child(div().text_sm().child("127.0.0.1"))
                            .child(div().w(px(100.)).child(Input::new(&self.port_input)))
                            .child({
                                let entity = entity.clone();
                                Button::new("save-port")
                                    .disabled(self.is_busy())
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
                                                this.status = "端口须为 1 到 65535。".into();
                                            }
                                            cx.notify();
                                        });
                                    },
                                )
                            })
                            .child(self.inbound_mode_buttons(
                                cx,
                                "mixed-mode",
                                self.strategy.mixed_mode,
                                Self::set_mixed_mode,
                            )),
                    )
                    .child(self.global_mode_row(
                        cx,
                        theme,
                        self.strategy.mixed_mode == InboundMode::Global,
                        "mixed",
                    )),
            ))
            .child(self.updates_panel(cx, theme))
            .child(panel(
                theme,
                "排除过滤器",
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("从订阅节点名里排除流量、剩余、到期这类字样。不影响已保存的组。"),
                    )
                    .child(Input::new(&self.filter_input))
                    .child({
                        let entity = entity.clone();
                        Button::new("save-filter")
                            .disabled(self.is_busy())
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
            .child(self.strategy_backup_panel(cx, theme))
            .child(self.logs_panel(theme))
            .child(self.developer_panel(cx, theme))
    }

    fn strategy_backup_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        panel(
            theme,
            "配置",
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("导出或导入整份策略，含订阅链接、节点组、规则和本机开关。导入会先留下一份备份。"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child({
                            let entity = entity.clone();
                            Button::new("export-strategy")
                                .label("导出")
                                .disabled(self.is_busy())
                                .on_click(move |_, window, app| {
                                    entity.update(app, |this, cx| {
                                        this.export_strategy(window, cx);
                                    });
                                })
                        })
                        .child({
                            let entity = entity.clone();
                            Button::new("import-strategy")
                                .label("导入")
                                .disabled(self.is_busy())
                                .on_click(move |_, window, app| {
                                    entity.update(app, |this, cx| {
                                        this.begin_import_strategy(window, cx);
                                    });
                                })
                        }),
                ),
        )
    }

    fn export_strategy(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            self.status = "正在处理上一项操作。".into();
            cx.notify();
            return;
        }
        if let Ok(port) = self.port_input.read(cx).value().trim().parse::<u16>() {
            self.strategy.mixed_port = port;
        }
        self.strategy.exclude_filter = self.filter_input.read(cx).value().to_string();
        let default_name = myproxy::strategy::default_export_name();
        match crate::file_dialog::save_strategy(&default_name) {
            Ok(crate::file_dialog::FileDialogChoice::Cancelled) => {}
            Ok(crate::file_dialog::FileDialogChoice::Path(path)) => {
                match myproxy::strategy::export_to(&self.strategy, &path) {
                    Ok(()) => self.status = format!("已导出到 {}", path.display()),
                    Err(error) => {
                        log::error("ui", format!("export strategy failed: {error:#}"));
                        self.status = format!("导出失败：{error:#}");
                    }
                }
            }
            Err(error) => self.status = format!("无法打开保存面板：{error:#}"),
        }
        cx.notify();
    }

    fn begin_import_strategy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            self.status = "正在处理上一项操作。".into();
            cx.notify();
            return;
        }
        if self.group_modal_open || self.rule_modal_open {
            self.status = "请先关闭编辑窗口再导入。".into();
            cx.notify();
            return;
        }
        let path = match crate::file_dialog::open_strategy() {
            Ok(crate::file_dialog::FileDialogChoice::Cancelled) => return,
            Ok(crate::file_dialog::FileDialogChoice::Path(path)) => path,
            Err(error) => {
                self.status = format!("无法打开文件面板：{error:#}");
                cx.notify();
                return;
            }
        };
        match myproxy::strategy::parse_import(&path) {
            Ok(preview) => self.open_import_confirm(path, preview, window, cx),
            Err(error) => {
                self.status = format!("无法读取配置：{error:#}");
                cx.notify();
            }
        }
    }

    fn open_import_confirm(
        &mut self,
        path: PathBuf,
        preview: Strategy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("配置")
            .to_string();
        let message = format!(
            "将用「{file_name}」整份替换当前策略（{} 个订阅、{} 个节点组、{} 条规则）。导入前会留下备份。",
            preview.subscriptions.len(),
            preview.groups.len(),
            preview.rule_sets.len()
        );
        let parent = cx.entity();
        window.close_all_dialogs(cx);
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("导入配置")
                .width(px(440.))
                .overlay_closable(true)
                .button_props(DialogButtonProps::default().on_ok({
                    let parent = parent.clone();
                    let path = path.clone();
                    move |_, _, cx| {
                        parent.update(cx, |this, cx| {
                            this.finish_import_strategy(&path, cx);
                        });
                        true
                    }
                }))
                .child(div().text_sm().child(message.clone()))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("import-strategy-cancel")
                                .label("取消")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(Cancel), cx)
                                }),
                        )
                        .child(
                            Button::new("import-strategy-ok")
                                .primary()
                                .label("导入")
                                .on_click(|_, window, cx| {
                                    window
                                        .dispatch_action(Box::new(Confirm { secondary: false }), cx)
                                }),
                        ),
                )
        });
    }

    fn finish_import_strategy(&mut self, path: &Path, cx: &mut Context<Self>) {
        if self.is_busy() {
            self.status = "正在处理上一项操作。".into();
            cx.notify();
            return;
        }
        match myproxy::strategy::import_from(path) {
            Ok(outcome) => self.adopt_imported(outcome, cx),
            Err(error) => {
                log::error("ui", format!("import strategy failed: {error:#}"));
                self.status = format!("导入失败：{error:#}");
                cx.notify();
            }
        }
    }

    fn adopt_imported(
        &mut self,
        outcome: myproxy::strategy::ImportOutcome,
        cx: &mut Context<Self>,
    ) {
        self.strategy = outcome.strategy.clone();
        self.saved = outcome.strategy.clone();
        if let Ok(path) = myproxy::paths::strategy_path() {
            self.strategy_stamp = file_stamp(&path);
        }
        self.external_change_pending = false;
        self.pending_port_input = Some(self.strategy.mixed_port.to_string());
        self.pending_filter_input = Some(self.strategy.exclude_filter.clone());
        log::set_developer(self.strategy.developer_mode);
        self.applied.update_channel = self.strategy.update_channel;
        crate::sparkle::set_channel(self.strategy.update_channel.unwrap_or_default());
        if let Err(error) = myproxy::login_item::sync(self.strategy.launch_at_login) {
            log::warn("login", format!("{error:#}"));
        }
        let backup_note = outcome
            .backup
            .as_ref()
            .map(|path| format!("备份：{}。", path.display()))
            .unwrap_or_default();
        if self.wanted {
            if self.start_apply(cx) {
                self.status = format!("已导入策略。{backup_note}正在应用…");
            }
        } else {
            self.status = format!("已导入策略。{backup_note}下次连接或应用后生效。");
            cx.notify();
        }
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
                        .child(
                            h_flex()
                                .gap_2()
                                .child({
                                    let entity = entity.clone();
                                    let mut button = Button::new("install-cli").small();
                                    button = if installed {
                                        button.label("已安装，更新链接")
                                    } else {
                                        button.primary().label("安装")
                                    };
                                    button.disabled(self.is_busy()).on_click(move |_, _, app| {
                                        entity.update(app, |this, cx| {
                                            match myproxy::cli_install::install() {
                                                Ok(path) => {
                                                    this.cli_installed = true;
                                                    this.status = format!(
                                                        "命令行工具已安装：{}",
                                                        path.display()
                                                    );
                                                }
                                                Err(error) => {
                                                    this.status = format!(
                                                        "命令行工具安装失败：{error:#}"
                                                    );
                                                }
                                            }
                                            cx.notify();
                                        });
                                    })
                                })
                                .child({
                                    let entity = entity.clone();
                                    Button::new("cli-install-help")
                                        .small()
                                        .label("安装说明")
                                        .on_click(move |_, window, app| {
                                            let entity = entity.clone();
                                            crate::onboard::open(window, app, move |result, cx| {
                                                entity.update(cx, |this, cx| {
                                                    match result {
                                                        Ok(path) => {
                                                            this.cli_installed = true;
                                                            this.status = format!(
                                                                "命令行工具已安装：{}",
                                                                path.display()
                                                            );
                                                        }
                                                        Err(error) => {
                                                            this.status = format!(
                                                                "命令行工具安装失败：{error}"
                                                            );
                                                        }
                                                    }
                                                    cx.notify();
                                                });
                                            });
                                        })
                                }),
                        ),
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

    fn inbound_modes_subtitle(&self) -> String {
        format!("已保存模式：Mixed {} · 系统接管 {}。规则模式先绕过本机/私网再匹配策略；代理、全局、直连绕过用户规则。TUN 固定按策略规则。{}",
            self.strategy.mixed_mode.label(), self.strategy.extension_mode.label(),
            if self.is_dirty() { " 当前有待应用修改。" } else { "" })
    }

    fn global_mode_row(
        &self,
        cx: &mut Context<Self>,
        theme: &Theme,
        active: bool,
        id_prefix: &str,
    ) -> impl IntoElement {
        let now = self.global_now();
        let now_label = global_selection_label(&now, self.connected);
        let entity = cx.entity();
        let muted_fg = theme.muted_foreground;
        let mut shortcuts: Vec<String> = vec!["DIRECT".into(), "REJECT".into()];
        for group in &self.strategy.groups {
            if !shortcuts.iter().any(|name| name == &group.name) {
                shortcuts.push(group.name.clone());
            }
        }
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(muted_fg).child(if active {
                format!("GLOBAL 当前 {now_label} · 点下方组或到节点组选节点")
            } else {
                format!("内置 GLOBAL 当前 {now_label} · 仅「全局」模式整段走它")
            }))
            .when(active, |this| {
                this.child(h_flex().w_full().flex_wrap().gap_1().children(
                    shortcuts.into_iter().map(|name| {
                        let entity = entity.clone();
                        let pick = name.clone();
                        let mut button =
                            Button::new(SharedString::from(format!("{id_prefix}-global-{name}")))
                                .small()
                                .label(name.clone());
                        if name == now {
                            button = button.primary();
                        }
                        button.disabled(self.is_busy()).on_click(move |_, _, app| {
                            entity.update(app, |this, cx| {
                                this.select_global_member(&pick, cx);
                            });
                        })
                    }),
                ))
            })
    }

    fn inbound_mode_buttons(
        &self,
        cx: &mut Context<Self>,
        id_prefix: &str,
        current: InboundMode,
        set: fn(&mut Self, InboundMode, &mut Context<Self>),
    ) -> impl IntoElement {
        let entity = cx.entity();
        h_flex()
            .gap_1()
            .flex_wrap()
            .children(InboundMode::ALL.into_iter().map(move |mode| {
                let entity = entity.clone();
                let mut btn =
                    Button::new(SharedString::from(format!("{id_prefix}-{}", mode.as_str())))
                        .small();
                btn = if current == mode {
                    btn.primary().label(mode.label())
                } else {
                    btn.label(mode.label())
                };
                btn.disabled(self.is_busy()).on_click(move |_, _, app| {
                    entity.update(app, |this, cx| {
                        set(this, mode, cx);
                        cx.notify();
                    });
                })
            }))
    }

    fn set_mixed_mode(&mut self, mode: InboundMode, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.strategy.mixed_mode = mode;
        if mode == InboundMode::Global {
            self.strategy.ensure_global_selected();
        }
        self.persist_inbound_mode(cx, format!("Mixed 已保存为{}。", mode.label()));
    }

    fn set_extension_mode(&mut self, mode: InboundMode, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.strategy.extension_mode = mode;
        if mode == InboundMode::Global {
            self.strategy.ensure_global_selected();
        }
        self.persist_inbound_mode(cx, format!("系统接管已保存为{}。", mode.label()));
    }

    fn persist_inbound_mode(&mut self, cx: &mut Context<Self>, note: String) {
        if self.wanted {
            if self.persist_and_apply(cx) {
                self.status = format!("{note}正在应用…");
            }
        } else if self.persist() {
            self.status = format!("{note}下次连接或应用后生效。");
        }
    }

    fn set_system_extension(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.strategy.system_extension = on;
        if on {
            self.strategy.tun = false;
        }
        if self.wanted {
            self.persist_and_apply(cx);
        } else if self.persist() {
            self.status = if on {
                format!(
                    "已记录。下次连接会请求系统扩展，请到 {} 允许 myproxy。",
                    setup::login_items_path_label()
                )
            } else {
                "已保存关闭系统接管的意图。".into()
            };
        }
    }

    fn set_tun(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.strategy.tun = on;
        if on {
            self.strategy.system_extension = false;
        }
        if self.wanted {
            self.persist_and_apply(cx);
        } else if self.persist() {
            self.status = if on {
                "已记录。下次连接走 TUN；首次会要管理员密码。".into()
            } else {
                "已保存关闭 TUN 的意图。".into()
            };
        }
    }

    fn routing_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let current = self.strategy.routing_profile;
        let unmatched = myproxy::compile::unmatched_target(&self.strategy);
        panel(
            theme,
            "分流",
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("规则入口先绕过本机与私网，再按用户规则、GFWList 或国内直连、未命中走向处理。GFWList 整包装卸。国内直连的 GEOIP 在 Mihomo 里求值，系统接管不做 IP 库。"),
                )
                .child(
                    h_flex().gap_1().flex_wrap().children(RoutingProfile::ALL.into_iter().map(
                        |profile| {
                            let entity = entity.clone();
                            let mut btn = Button::new(SharedString::from(format!(
                                "routing-{}",
                                profile.as_str()
                            )))
                            .small();
                            btn = if current == profile {
                                btn.primary().label(profile.label())
                            } else {
                                btn.label(profile.label())
                            };
                            btn.disabled(self.is_busy()).on_click(move |_, _, app| {
                                entity.update(app, |this, cx| {
                                    this.set_routing_profile(profile, cx);
                                    cx.notify();
                                });
                            })
                        },
                    )),
                )
                .when(
                    matches!(current, RoutingProfile::Group | RoutingProfile::Chinadirect),
                    |this| {
                    this.child(
                        h_flex().gap_1().flex_wrap().children(
                            self.strategy.groups.iter().map(|group| {
                                let name = group.name.clone();
                                let selected = unmatched == name;
                                let entity = entity.clone();
                                let mut btn = Button::new(SharedString::from(format!(
                                    "routing-via-{name}"
                                )))
                                .small();
                                btn = if selected {
                                    btn.primary().label(name.clone())
                                } else {
                                    btn.label(name.clone())
                                };
                                btn.disabled(self.is_busy()).on_click(move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.strategy.unmatched_via = name.clone();
                                        this.persist_inbound_mode(
                                            cx,
                                            format!("未匹配走 {name}。"),
                                        );
                                        cx.notify();
                                    });
                                })
                            }),
                        ),
                    )
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(match current {
                            RoutingProfile::Allowlist => {
                                "未命中直连。Telegram 和其他自己的规则仍按各自走向。".to_string()
                            }
                            RoutingProfile::Gfwlist => {
                                "已装 Loyalsoldier GFWList，列表内走默认组，其余直连。换回未命中直连即卸下。".to_string()
                            }
                            RoutingProfile::Group => {
                                format!("未命中走 {unmatched}。连接或应用后生效。")
                            }
                            RoutingProfile::Chinadirect => {
                                format!("中国大陆 IP 直连（GEOIP,CN）。其余走 {unmatched}。未单独命中的接管流量在 Mihomo 里做 GEOIP。")
                            }
                        }),
                ),
        )
    }

    fn set_routing_profile(&mut self, profile: RoutingProfile, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.strategy.set_routing_profile(profile);
        self.persist_inbound_mode(cx, format!("分流已保存为{}。", profile.label()));
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
                            Switch::new("se-toggle")
                                .small()
                                .label(if on { "开启" } else { "关闭" })
                                .checked(on)
                                .accessibility_label("系统接管")
                                .disabled(self.is_busy())
                                .on_click({
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
                            format!(
                                "已保存启用意图；实际状态见下方。请到 {} 允许 myproxy。",
                                setup::login_items_path_label()
                            )
                        } else {
                            "关闭系统接管后，显式代理客户端仍可使用 Mixed；TUN 单独控制。".into()
                        }),
                )
                .child({
                    let entity = entity.clone();
                    let waiting = matches!(
                        self.extension_status.phase,
                        Phase::WaitingApproval | Phase::Requesting
                    );
                    let mut button = Button::new("open-login-items")
                        .small()
                        .label("打开系统设置");
                    if waiting {
                        button = button.primary();
                    }
                    button.on_click(move |_, _, app| {
                        entity.update(app, |this, cx| {
                            match network_extension::open_login_items_settings() {
                                Ok(()) => {
                                    this.status = format!(
                                        "已打开系统设置。到 {} 允许 myproxy。",
                                        setup::login_items_path_label()
                                    );
                                }
                                Err(error) => {
                                    this.status = format!(
                                        "无法打开系统设置：{error:#}。请手动到 {}。",
                                        setup::login_items_path_label()
                                    );
                                }
                            }
                            cx.notify();
                        });
                    })
                })
                .child(div().text_xs().child(format!("系统接管：{} · DNS：{}", self.extension_status.phase_label(), self.extension_status.dns_label())))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!(
                            "捕获 {} · 失败开放 {}",
                            if self.extension_status.capture_enabled { "开" } else { "关" },
                            if self.extension_status.fail_open { "开" } else { "关" }
                        )),
                )
                .when_some(self.extension_status.message.clone(), |this, message| this.child(div().text_xs().text_color(theme_ext::warning_text(theme)).child(message)))
                .when_some(self.extension_status.dns_message.clone(), |this, message| this.child(div().text_xs().text_color(theme_ext::warning_text(theme)).child(format!("DNS：{message}"))))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if self.strategy.lan_capture {
                            "内置旁路：本机组件、回环、链路本地、.local。局域网地址会进规则。"
                        } else {
                            "内置旁路：本机组件、回环、私网、链路本地、.local。这些项不能删除。"
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
                                .child(div().text_sm().child("局域网也进规则"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("关闭私网旁路。回环和本机组件仍直连。"),
                                ),
                        )
                        .child({
                            let entity = entity.clone();
                            let lan_on = self.strategy.lan_capture;
                            Switch::new("lan-capture-toggle")
                                .small()
                                .label(if lan_on { "开启" } else { "关闭" })
                                .checked(lan_on)
                                .accessibility_label("局域网也进规则")
                                .disabled(self.is_busy())
                                .on_click(move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.strategy.lan_capture = !this.strategy.lan_capture;
                                        this.persist_inbound_mode(
                                            cx,
                                            if this.strategy.lan_capture {
                                                "局域网将进入规则。".into()
                                            } else {
                                                "局域网恢复内置旁路。".into()
                                            },
                                        );
                                        cx.notify();
                                    });
                                })
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
                                .child(div().text_sm().child("系统代理"))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child("把系统 HTTP/HTTPS/SOCKS 指到 Mixed。未开接管、未开此项时浏览器需手填端口。与系统接管、TUN 三选一即可。断开或退出会恢复。"),
                                ),
                        )
                        .child({
                            let entity = entity.clone();
                            let proxy_on = self.strategy.system_proxy;
                            Switch::new("system-proxy-toggle")
                                .small()
                                .label(if proxy_on { "开启" } else { "关闭" })
                                .checked(proxy_on)
                                .accessibility_label("系统代理")
                                .disabled(self.is_busy())
                                .on_click(move |_, _, app| {
                                    entity.update(app, |this, cx| {
                                        this.strategy.system_proxy = !this.strategy.system_proxy;
                                        this.persist_inbound_mode(
                                            cx,
                                            if this.strategy.system_proxy {
                                                "系统代理将指向 Mixed。".into()
                                            } else {
                                                "将恢复原先的系统代理。".into()
                                            },
                                        );
                                        cx.notify();
                                    });
                                })
                        }),
                )
                .child(self.inbound_mode_buttons(
                    cx,
                    "extension-mode",
                    self.strategy.extension_mode,
                    Self::set_extension_mode,
                ))
                .child(self.global_mode_row(
                    cx,
                    theme,
                    self.strategy.extension_mode == InboundMode::Global,
                    "extension",
                ))
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
                                        .child("TUN 与系统接管互斥，固定按策略规则运行，不使用 Mixed 或接管的模式按钮。"),
                                ),
                        )
                        .child({
                            let entity = entity.clone();
                            Switch::new("tun-toggle")
                                .small()
                                .label(if tun_on { "开启" } else { "关闭" })
                                .checked(tun_on)
                                .accessibility_label("TUN")
                                .disabled(self.is_busy())
                                .on_click(move |_, _, app| {
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
                        let previous = this.strategy.launch_at_login;
                        this.strategy.launch_at_login = !previous;
                        if this.persist() {
                            if let Err(err) =
                                myproxy::login_item::sync(this.strategy.launch_at_login)
                            {
                                log::warn("login", format!("{err:#}"));
                                this.status = format!("已保存。开机启动未生效：{err}");
                            }
                        } else {
                            this.strategy.launch_at_login = previous;
                        }
                        cx.notify();
                    },
                ))
                .when(!bundled, |this| {
                    this.child(div().text_xs().text_color(theme.muted_foreground).child(
                        "当前不是 .app，登录项不会注册。安装到 /Applications/myproxy.app 后生效。",
                    ))
                })
                .child(self.flag_row(
                    entity.clone(),
                    theme,
                    "silent-launch",
                    "静默启动",
                    "启动时不显示主窗口，可从菜单栏打开。",
                    self.strategy.silent_launch,
                    |this, cx| {
                        let previous = this.strategy.silent_launch;
                        this.strategy.silent_launch = !previous;
                        if !this.persist() {
                            this.strategy.silent_launch = previous;
                        }
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
                        let previous = this.strategy.lite_mode;
                        this.strategy.lite_mode = !previous;
                        if !this.persist() {
                            this.strategy.lite_mode = previous;
                        }
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
                        let previous = this.strategy.connect_on_launch;
                        this.strategy.connect_on_launch = !previous;
                        if !this.persist() {
                            this.strategy.connect_on_launch = previous;
                        }
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
                Switch::new(id)
                    .small()
                    .label(if on { "开启" } else { "关闭" })
                    .checked(on)
                    .accessibility_label(title)
                    .disabled(self.is_busy())
                    .on_click(move |_, _, app| {
                        entity.update(app, |this, cx| {
                            if !this.is_busy() {
                                apply(this, cx);
                            }
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
        let version = updates::VERSION;
        let channel = self.strategy.update_channel.unwrap_or_default();
        let hint = match channel {
            UpdateChannel::Prod => {
                "仅接收正式版本。切回后，会在发布比当前版本更新的正式版时更新。相邻正式版走增量包。"
            }
            UpdateChannel::Nightly => {
                "接收 main 分支的每日构建，可能包含尚未稳定的改动。相邻 Nightly 走增量包。"
            }
        };
        panel(
            theme,
            "更新",
            v_flex()
                .gap_2()
                .child(div().text_sm().child(format!("当前版本 {version}")))
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
                                .child(
                                    Button::new("update-prod")
                                        .label(UpdateChannel::Prod.label())
                                        .selected(channel == UpdateChannel::Prod)
                                        .disabled(self.is_busy()),
                                )
                                .child(
                                    Button::new("update-nightly")
                                        .label(UpdateChannel::Nightly.label())
                                        .selected(channel == UpdateChannel::Nightly)
                                        .disabled(self.is_busy()),
                                )
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
                                            this.status =
                                                format!("更新通道已切换为{}。", next.label());
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
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("已连接时，检查更新与下载走 Mixed；未连接则直连。"),
                )
                .when(!crate::sparkle::available(), |this| {
                    this.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("此开发构建不支持应用内更新。安装发布版后可按所选通道更新。"),
                    )
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

    fn logs_panel(&self, theme: &Theme) -> impl IntoElement {
        let log_path = log::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(无法创建日志文件)".into());
        panel(
            theme,
            "日志",
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Info、Warning、Error 始终写入同一文件。Debug / Trace 仅开发者模式或 MYPROXY_DEV=1。不记录订阅 URL。"),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .flex_wrap()
                        .child(
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .text_color(theme.muted_foreground)
                                .child(log_path),
                        )
                        .child({
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
                        }),
                )
                .child(
                    v_flex()
                        .id("app-log")
                        .max_h(px(240.))
                        .overflow_y_scroll()
                        .p_3()
                        .gap_1()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.group_box)
                        .children(log::recent(80).into_iter().map(|line| {
                            let color = match log::Level::from_line(&line) {
                                Some(log::Level::Error) | Some(log::Level::Warn) => {
                                    Some(theme_ext::warning_text(theme))
                                }
                                Some(log::Level::Debug) | Some(log::Level::Trace) => {
                                    Some(theme.muted_foreground)
                                }
                                _ => None,
                            };
                            div()
                                .text_xs()
                                .font_family(theme.mono_font_family.clone())
                                .when_some(color, |this, color| this.text_color(color))
                                .child(line)
                        })),
                ),
        )
    }

    fn developer_panel(&self, cx: &mut Context<Self>, theme: &Theme) -> impl IntoElement {
        let entity = cx.entity();
        let on = self.strategy.developer_mode || log::env_forced();
        panel(
            theme,
            "开发者",
            v_flex()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("打开后，Debug / Trace 也会写入上方日志。MYPROXY_DEV=1 同样生效。"),
                )
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
    h_flex()
        .gap_1()
        .flex_wrap()
        .children(tokens.iter().map(|token| {
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
        .when(pinned, |this| this.child(pill(theme, "钉住")))
        .when(blocked, |this| this.child(pill(theme, "排除")))
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

fn global_selection_label(now: &str, connected: bool) -> String {
    if now.is_empty() {
        if connected {
            "等待核心状态"
        } else {
            "未指定"
        }
        .into()
    } else if connected {
        now.to_string()
    } else {
        format!("{now}（已保存）")
    }
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
    count: Option<usize>,
    selected: bool,
    now: &str,
    members: &[(String, Option<u32>)],
    can_select: bool,
    delaying: bool,
    connected: bool,
    limit: usize,
    busy: bool,
) -> impl IntoElement {
    let id = group.id.clone();
    let del_id = group.id.clone();
    let group_name = group.name.clone();
    let muted_fg = theme.muted_foreground;
    let hover = theme_ext::row_hover(theme);
    let selected_border = theme_ext::selection_border(theme);
    let shown: Vec<_> = members.iter().take(limit).cloned().collect();
    v_flex()
        .id(SharedString::from(format!("group-card-{id}")))
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(if selected { selected_border } else { theme.border })
        .bg(theme.group_box)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
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
                .child(div().text_sm().font_semibold().child(format!(
                        "{}  ·  {}  ·  {}",
                        group.name,
                        group.kind_label(),
                        count
                            .map(|count| format!("{count} 个节点"))
                            .unwrap_or_else(|| "等待核心状态".into())
                    )))
                .child(
                    h_flex()
                        .gap_1()
                        .child({
                            let entity = entity.clone();
                            let id = id.clone();
                            Button::new(SharedString::from(format!("edit-group-{id}")))
                                .small()
                                .label("编辑")
                                .accessibility_label(format!("编辑节点组 {}", group_name))
                                .on_click(move |_, window, app| {
                                    app.stop_propagation();
                                    entity.update(app, |this, cx| {
                                        this.open_group_dialog(Some(&id), window, cx);
                                        cx.notify();
                                    });
                                })
                        })
                        .child({
                            let entity = entity.clone();
                            let name = group_name.clone();
                            Button::new(SharedString::from(format!("delay-group-{id}")))
                                .small()
                                .label(if delaying {
                                    "测延迟…"
                                } else {
                                    "测延迟"
                                })
                                .disabled(delaying || !connected || busy)
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
                                .disabled(busy)
                                .on_click(move |_, window, app| {
                                    app.stop_propagation();
                                    let close_modal = entity.read(app).group_edit_id.as_deref()
                                        == Some(del_id.as_str());
                                    let removed = entity.update(app, |this, cx| {
                                        if this.is_busy() {
                                            return false;
                                        }
                                        let previous = this.strategy.clone();
                                        if let Err(err) =
                                            this.strategy.remove_group_checked(&del_id)
                                        {
                                            this.status = format!("无法删除节点组：{err:#}");
                                            cx.notify();
                                            return false;
                                        }
                                        let applied = this.persist_and_apply(cx);
                                        if applied && close_modal {
                                            this.group_modal_open = false;
                                            this.group_edit_id = None;
                                        } else if !applied {
                                            this.strategy = previous;
                                        }
                                        cx.notify();
                                        applied
                                    });
                                    if close_modal && removed {
                                        window.close_dialog(app);
                                    }
                                })
                        }),
                ),
        )
        .child(div().text_xs().text_color(muted_fg).child(format!(
            "{} {}  ·  {}",
            if connected {
                "核心当前"
            } else {
                "已保存选择"
            },
            now,
            group.policy_label()
        )))
        .when(!shown.is_empty(), |this| {
            this.child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_1()
                    .children(shown.into_iter().map(|(name, delay)| {
                        let delay_text = format_delay(delay);
                        let label = if delay_text.is_empty() {
                            name.clone()
                        } else {
                            format!("{name}  {delay_text}")
                        };
                        let is_now = name == now;
                        let entity = entity.clone();
                        let group_id = id.clone();
                        if can_select {
                            Button::new(SharedString::from(format!("pick-{id}-{name}")))
                                .small()
                                .label(label)
                                .max_w_full()
                                .overflow_hidden()
                                .tooltip(name.clone())
                                .selected(is_now)
                                .disabled(busy)
                                .accessibility_label(format!("选择 {name}"))
                                .on_click(move |_, _, app| {
                                    app.stop_propagation();
                                    entity.update(app, |this, cx| {
                                        this.select_group_member(&group_id, &name, cx)
                                    });
                                })
                                .into_any_element()
                        } else {
                            div()
                                .max_w_full()
                                .overflow_hidden()
                                .text_ellipsis()
                                .px_2()
                                .py_1()
                                .rounded(theme.radius)
                                .text_xs()
                                .bg(if is_now {
                                    theme_ext::chip_bg(theme)
                                } else {
                                    theme.muted
                                })
                                .child(if is_now {
                                    format!("{label} · 当前")
                                } else {
                                    label
                                })
                                .into_any_element()
                        }
                    })),
            )
        })
        .when(members.len() > limit, |this| {
            let entity = entity.clone();
            let id = id.clone();
            this.child(
                Button::new(SharedString::from(format!("more-{id}")))
                    .small()
                    .label(format!("继续显示（{} / {}）", limit, members.len()))
                    .on_click(move |_, _, app| {
                        app.stop_propagation();
                        entity.update(app, |this, cx| {
                            *this.member_limits.entry(id.clone()).or_insert(36) += 36;
                            cx.notify();
                        });
                    }),
            )
        })
}

fn render_global_card(
    entity: Entity<AppView>,
    theme: &Theme,
    now: &str,
    members: &[(String, Option<u32>)],
    delaying: bool,
    connected: bool,
    inbound_global: bool,
    query: &Entity<InputState>,
    limit: usize,
    busy: bool,
) -> impl IntoElement {
    let now_label = global_selection_label(now, connected);
    let muted_fg = theme.muted_foreground;
    let hover = theme_ext::row_hover(theme);
    let selected_border = theme_ext::selection_border(theme);
    let shown: Vec<_> = members.iter().take(limit).cloned().collect();
    let shown_empty = shown.is_empty();
    v_flex()
        .id("group-card-GLOBAL")
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(if inbound_global { selected_border } else { theme.border })
        .bg(theme.group_box)
        .hover(move |style| style.bg(hover))
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(format!("GLOBAL  ·  内置选择  ·  {} 个成员", members.len())),
                )
                .child({
                    let entity = entity.clone();
                    Button::new("delay-group-GLOBAL")
                        .small()
                        .label(if delaying {
                            "测延迟…"
                        } else {
                            "测延迟"
                        })
                        .disabled(delaying || !connected || busy)
                        .on_click(move |_, _, app| {
                            entity.update(app, |this, cx| {
                                this.start_group_delay(GLOBAL_GROUP, cx);
                            });
                        })
                }),
        )
        .child(
            div()
                .text_xs()
                .text_color(muted_fg)
                .child(if inbound_global {
                    format!("当前 {now_label}  ·  Mixed 或接管为全局时整段走这里")
                } else {
                    format!("当前 {now_label}  ·  未开全局时只作备用，规则仍按组走")
                }),
        )
        .child(div().w_full().child(Input::new(query)))
        .when(!shown_empty, |this| {
            this.child(
                h_flex()
                    .w_full()
                    .flex_wrap()
                    .gap_1()
                    .children(shown.into_iter().map(|(name, delay)| {
                        let delay_text = format_delay(delay);
                        let label = if delay_text.is_empty() {
                            name.clone()
                        } else {
                            format!("{name}  {delay_text}")
                        };
                        let is_now = name == now;
                        let entity = entity.clone();
                        let mut button =
                            Button::new(SharedString::from(format!("pick-GLOBAL-{name}")))
                                .small()
                                .label(label)
                                .max_w_full()
                                .overflow_hidden()
                                .tooltip(name.clone())
                                .disabled(busy)
                                .selected(is_now);
                        if is_now {
                            button = button.primary();
                        }
                        button.on_click(move |_, _, app| {
                            entity.update(app, |this, cx| {
                                this.select_global_member(&name, cx);
                            });
                        })
                    })),
            )
        })
        .when(shown_empty, |this| {
            this.child(
                div()
                    .text_xs()
                    .text_color(muted_fg)
                    .child("没有匹配的成员。"),
            )
        })
        .when(members.len() > limit, |this| {
            let entity = entity.clone();
            this.child(
                Button::new("more-GLOBAL")
                    .small()
                    .label(format!("继续显示（{} / {}）", limit, members.len()))
                    .on_click(move |_, _, app| {
                        entity.update(app, |this, cx| {
                            this.global_limit += 36;
                            cx.notify();
                        });
                    }),
            )
        })
}

fn file_stamp(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

fn empty_hint_box(theme: &Theme) -> Div {
    div()
        .p_4()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
}

fn empty_hint(theme: &Theme, text: &str) -> impl IntoElement {
    empty_hint_box(theme)
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(text.to_string())
}

fn empty_hint_action(
    theme: &Theme,
    text: &str,
    action: impl IntoElement,
) -> impl IntoElement {
    empty_hint_box(theme).child(
        v_flex()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(text.to_string()),
            )
            .child(action),
    )
}

fn connection_header_row(
    entity: Entity<AppView>,
    theme: &Theme,
    connections: &[controller::LiveConnection],
    filters: &ConnectionFilters,
) -> impl IntoElement {
    let muted_fg = theme.muted_foreground;
    h_flex()
        .w_full()
        .items_center()
        .px_3()
        .py_1()
        .gap_2()
        .child(connection_filter_header(
            entity.clone(),
            theme,
            "conn-filter-process",
            Some(px(108.)),
            ConnectionColumn::Process,
            "进程",
            controller::connection_column_values(connections, filters, ConnectionColumn::Process),
            filters.process.clone(),
        ))
        .child(connection_filter_header(
            entity.clone(),
            theme,
            "conn-filter-destination",
            None,
            ConnectionColumn::Destination,
            "目标",
            controller::connection_column_values(
                connections,
                filters,
                ConnectionColumn::Destination,
            ),
            filters.destination.clone(),
        ))
        .child(connection_filter_header(
            entity.clone(),
            theme,
            "conn-filter-network",
            Some(px(64.)),
            ConnectionColumn::Network,
            "协议",
            controller::connection_column_values(connections, filters, ConnectionColumn::Network),
            filters.network.clone(),
        ))
        .child(connection_filter_header(
            entity,
            theme,
            "conn-filter-chain",
            Some(px(168.)),
            ConnectionColumn::Chain,
            "走向",
            controller::connection_column_values(connections, filters, ConnectionColumn::Chain),
            filters.chain.clone(),
        ))
        .child(connection_col(px(72.), "上传", muted_fg, None))
        .child(connection_col(px(72.), "下载", muted_fg, None))
        .child(connection_col(px(80.), "时长", muted_fg, None))
        .child(div().w(px(56.)))
}

fn connection_filter_header(
    entity: Entity<AppView>,
    theme: &Theme,
    id: &'static str,
    width: Option<Pixels>,
    column: ConnectionColumn,
    title: &'static str,
    mut values: Vec<String>,
    current: Option<String>,
) -> impl IntoElement {
    if let Some(current) = &current {
        if !values.iter().any(|value| value == current) {
            values.insert(0, current.clone());
        }
    }
    let active = current.is_some();
    let color = if active {
        theme.primary
    } else {
        theme.muted_foreground
    };
    let button = Button::new(id)
        .text()
        .small()
        .label(title)
        .icon(IconName::ChevronDown)
        .text_color(color)
        .dropdown_menu({
            let entity = entity.clone();
            let current = current.clone();
            move |menu, _, _| {
                let mut menu = menu.scrollable(true).min_w(px(168.));
                let clear_entity = entity.clone();
                menu = menu.item(
                    PopupMenuItem::new("全部")
                        .checked(current.is_none())
                        .on_click(move |_, _, app| {
                            clear_entity.update(app, |this, cx| {
                                this.connection_filters.set_column(column, None);
                                cx.notify();
                            });
                        }),
                );
                if !values.is_empty() {
                    menu = menu.separator();
                }
                for value in &values {
                    let checked = current.as_deref() == Some(value.as_str());
                    let pick_entity = entity.clone();
                    let picked = value.clone();
                    menu = menu.item(PopupMenuItem::new(value.clone()).checked(checked).on_click(
                        move |_, _, app| {
                            pick_entity.update(app, |this, cx| {
                                this.connection_filters
                                    .set_column(column, Some(picked.clone()));
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            }
        });
    div()
        .when_some(width, |this, width| this.w(width).min_w(width))
        .when(width.is_none(), |this| this.flex_1().min_w(px(96.)))
        .child(button)
}

fn connection_col(
    width: Pixels,
    text: impl Into<String>,
    color: Hsla,
    mono: Option<SharedString>,
) -> impl IntoElement {
    let text = text.into();
    let full_text = text.clone();
    div()
        .id(SharedString::from(format!("cell-{text}")))
        .overflow_hidden()
        .text_ellipsis()
        .tooltip(move |window, cx| Tooltip::new(full_text.clone()).build(window, cx))
        .w(width)
        .min_w(width)
        .flex_shrink_0()
        .text_xs()
        .text_color(color)
        .when_some(mono, |this, family| this.font_family(family))
        .child(text)
}

fn connection_col_flex(
    text: impl Into<String>,
    color: Hsla,
    mono: Option<SharedString>,
) -> impl IntoElement {
    let text = text.into();
    let full_text = text.clone();
    div()
        .id(SharedString::from(format!("cell-{text}")))
        .overflow_hidden()
        .text_ellipsis()
        .tooltip(move |window, cx| Tooltip::new(full_text.clone()).build(window, cx))
        .flex_1()
        .min_w(px(96.))
        .text_xs()
        .text_color(color)
        .when_some(mono, |this, family| this.font_family(family))
        .child(text)
}

fn render_connection_row(
    entity: Entity<AppView>,
    theme: &Theme,
    conn: &controller::LiveConnection,
    via_choices: Vec<ViaChoice>,
    busy: bool,
) -> impl IntoElement {
    let id = conn.id.clone();
    let process = conn.process.clone();
    let app_matcher = conn.app_matcher.clone();
    let hover = theme_ext::row_hover(theme);
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
        .hover(move |style| style.bg(hover))
        .context_menu({
            let entity = entity.clone();
            let process = process.clone();
            let app_matcher = app_matcher.clone();
            move |menu, window, cx| {
                let create_entity = entity.clone();
                let create_process = process.clone();
                let create_matcher = app_matcher.clone();
                menu.min_w(px(168.))
                    .item(
                        PopupMenuItem::new("对此进程建规则").on_click(move |_, window, app| {
                            create_entity.update(app, |this, cx| {
                                this.open_rule_dialog_for_process(
                                    &create_process,
                                    &create_matcher,
                                    window,
                                    cx,
                                );
                                cx.notify();
                            });
                        }),
                    )
                    .submenu("改为走向", window, cx, {
                        let entity = entity.clone();
                        let process = process.clone();
                        let app_matcher = app_matcher.clone();
                        let choices = via_choices.clone();
                        move |menu, _, _| {
                            let entity = entity.clone();
                            let process = process.clone();
                            let app_matcher = app_matcher.clone();
                            via_menu(menu, &choices, "", move |app, value| {
                                entity.update(app, |this, cx| {
                                    this.set_connection_process_via(
                                        &process,
                                        &app_matcher,
                                        &value,
                                        cx,
                                    );
                                    cx.notify();
                                });
                            })
                        }
                    })
            }
        })
        .child(connection_col(px(108.), conn.process.clone(), fg, None))
        .child(connection_col_flex(
            conn.destination.clone(),
            muted_fg,
            Some(mono.clone()),
        ))
        .child(connection_col(
            px(64.),
            conn.network.clone(),
            muted_fg,
            None,
        ))
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
                .disabled(busy)
                .accessibility_label(format!("关闭 {} 的连接", conn.destination))
                .on_click(move |_, _, app| {
                    entity.update(app, |this, cx| this.close_connections(Some(id.clone()), cx));
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
) -> impl IntoElement {
    let id = set.id.clone();
    let via = set.via.clone();
    let can_up = index > 0;
    let can_down = index + 1 < total;
    let muted_fg = theme.muted_foreground;
    let hover = theme_ext::row_hover(theme);
    let selected_border = theme_ext::selection_border(theme);
    const CHIP_LIMIT: usize = 10;
    let extra = set.matchers.len().saturating_sub(CHIP_LIMIT);
    v_flex()
        .id(SharedString::from(format!("rule-card-{id}")))
        .p_4()
        .gap_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(if selected { selected_border } else { theme.border })
        .bg(theme.group_box)
        .cursor_pointer()
        .hover(move |style| style.bg(hover))
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
                    .item(PopupMenuItem::new("上移").disabled(!can_up).on_click(
                        move |_, _, app| {
                            up_entity.update(app, |this, cx| {
                                this.move_selected_rule(&up_id, -1, cx);
                                cx.notify();
                            });
                        },
                    ))
                    .item(PopupMenuItem::new("下移").disabled(!can_down).on_click(
                        move |_, _, app| {
                            down_entity.update(app, |this, cx| {
                                this.move_selected_rule(&down_id, 1, cx);
                                cx.notify();
                            });
                        },
                    ))
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
                        .child(div().text_sm().font_semibold().child(set.name.clone()))
                        .child(pill(theme, &via_label(&via))),
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
                    this.child(div().text_xs().text_color(muted_fg).child("没有匹配项"))
                }),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child({
                    let entity = entity.clone();
                    let id = id.clone();
                    let name = set.name.clone();
                    Button::new(SharedString::from(format!("edit-rule-{id}")))
                        .small()
                        .label("编辑")
                        .accessibility_label(format!("编辑规则 {name}"))
                        .on_click(move |_, window, app| {
                            app.stop_propagation();
                            entity.update(app, |this, cx| {
                                this.open_rule_dialog(Some(&id), window, cx);
                                cx.notify();
                            });
                        })
                })
                .child({
                    let entity = entity.clone();
                    let id = id.clone();
                    let name = set.name.clone();
                    Button::new(SharedString::from(format!("up-rule-{id}")))
                        .small()
                        .label("上移")
                        .accessibility_label(format!("上移规则 {name}"))
                        .disabled(!can_up)
                        .on_click(move |_, _, app| {
                            app.stop_propagation();
                            entity.update(app, |this, cx| {
                                this.move_selected_rule(&id, -1, cx);
                                cx.notify();
                            });
                        })
                })
                .child({
                    let entity = entity.clone();
                    let id = id.clone();
                    let name = set.name.clone();
                    Button::new(SharedString::from(format!("down-rule-{id}")))
                        .small()
                        .label("下移")
                        .accessibility_label(format!("下移规则 {name}"))
                        .disabled(!can_down)
                        .on_click(move |_, _, app| {
                            app.stop_propagation();
                            entity.update(app, |this, cx| {
                                this.move_selected_rule(&id, 1, cx);
                                cx.notify();
                            });
                        })
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
        .min_w(px(140.))
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

fn pill(theme: &Theme, text: &str) -> impl IntoElement {
    div()
        .px_2()
        .py(px(2.))
        .rounded(px(999.))
        .bg(theme_ext::chip_bg(theme))
        .text_color(theme_ext::chip_fg(theme))
        .text_xs()
        .child(text.to_string())
}

fn status_dot(theme: &Theme, on: bool) -> impl IntoElement {
    div().size_2().rounded_full().bg(if on {
        theme.success
    } else {
        theme.muted_foreground
    })
}
