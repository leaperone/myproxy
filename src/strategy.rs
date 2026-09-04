use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::log;
use crate::paths;

pub const DEFAULT_EXCLUDE: &str =
    r"(?i)(流量|剩余|到期|官网|重置|过期|剩余流量|套餐到期|过期时间)";

pub const STRATEGY_SCHEMA: u32 = 5;

pub const TELEGRAM_GROUP: &str = "Telegram";

/// HTTP(S) hosts Telegram's web stack uses. These never match raw MTProto.
pub const TELEGRAM_SUFFIXES: &[&str] = &[
    "telegram.org",
    "t.me",
    "telegram.me",
    "telegra.ph",
    "telegram-cdn.org",
    "telesco.pe",
];

/// Published DC ranges from https://core.telegram.org/resources/cidr.txt plus 95.161/20.
/// MTProto often uses these IPs with an empty SNI/host.
pub const TELEGRAM_CIDRS: &[&str] = &[
    "91.108.56.0/22",
    "91.108.4.0/22",
    "91.108.8.0/22",
    "91.108.16.0/22",
    "91.108.12.0/22",
    "149.154.160.0/20",
    "91.105.192.0/23",
    "91.108.20.0/22",
    "185.76.151.0/24",
    "95.161.64.0/20",
    "2001:b28:f23d::/48",
    "2001:b28:f23f::/48",
    "2001:67c:4e8::/48",
    "2001:b28:f23c::/48",
    "2a0a:f280::/32",
];

fn telegram_matchers() -> Vec<Matcher> {
    let mut matchers = vec![
        Matcher::keyword("telegram".into()),
        Matcher::app("Telegram".into()),
        Matcher::app("ru.keepcoder.telegram".into()),
    ];
    for suffix in TELEGRAM_SUFFIXES {
        matchers.push(Matcher::suffix((*suffix).into()));
    }
    for cidr in TELEGRAM_CIDRS {
        matchers.push(Matcher::cidr((*cidr).into()));
    }
    matchers
}

fn telegram_rule_set() -> RuleSet {
    RuleSet {
        id: Uuid::new_v4().to_string(),
        name: TELEGRAM_GROUP.into(),
        via: TELEGRAM_GROUP.into(),
        matchers: telegram_matchers(),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Strategy {
    #[serde(default)]
    pub schema: u32,
    pub mixed_port: u16,
    pub exclude_filter: String,
    /// When true, myproxy writes debug traces to `myproxy.log` and the Settings page.
    #[serde(default)]
    pub developer_mode: bool,
    /// When true, compile mihomo TUN so apps need not point HTTP/SOCKS at Mixed.
    #[serde(default)]
    pub tun: bool,
    /// Register a macOS login item when running from a bundled `.app`.
    #[serde(default)]
    pub launch_at_login: bool,
    /// Do not show the main window on process start.
    #[serde(default)]
    pub silent_launch: bool,
    /// Do not construct the main window until the tray asks; hide the Dock icon.
    #[serde(default)]
    pub lite_mode: bool,
    /// Call `Supervisor::connect` during process start.
    #[serde(default)]
    pub connect_on_launch: bool,
    #[serde(default)]
    pub subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default)]
    pub rule_sets: Vec<RuleSet>,
    /// Legacy flat rules. Read on migrate, never written back.
    #[serde(default, skip_serializing)]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub id: String,
    pub name: String,
    #[serde(default = "default_select")]
    pub kind: String,
    /// When true, every kept catalog node in `sources` (or all sources) is a member.
    #[serde(default)]
    pub all_nodes: bool,
    /// Subscription display names. Empty means any subscription.
    #[serde(default)]
    pub sources: Vec<String>,
    /// Case-insensitive substrings or `*` `?` wildcards, OR'd. Ignored when `all_nodes` is true.
    #[serde(default)]
    pub name_contains: Vec<String>,
    /// Case-insensitive patterns, OR'd. Drops automatic matches only; pins stay.
    #[serde(default)]
    pub name_excludes: Vec<String>,
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Legacy single regex. Read on migrate, never written back.
    #[serde(default, skip_serializing)]
    pub filter: String,
}

fn default_select() -> String {
    "select".into()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    #[serde(default)]
    pub app: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub keyword: String,
    pub via: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Matcher {
    pub kind: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleSet {
    pub id: String,
    pub name: String,
    pub via: String,
    #[serde(default)]
    pub matchers: Vec<Matcher>,
}

impl Default for Strategy {
    fn default() -> Self {
        Self {
            schema: STRATEGY_SCHEMA,
            mixed_port: 17890,
            exclude_filter: DEFAULT_EXCLUDE.into(),
            developer_mode: false,
            tun: false,
            launch_at_login: false,
            silent_launch: false,
            lite_mode: false,
            connect_on_launch: false,
            subscriptions: Vec::new(),
            groups: vec![
                Group::all_nodes("PROXY".into(), "select".into()),
                Group::all_nodes(TELEGRAM_GROUP.into(), "select".into()),
            ],
            rule_sets: vec![telegram_rule_set()],
            rules: Vec::new(),
        }
    }
}

impl Strategy {
    pub fn load() -> Result<Self> {
        let path = paths::strategy_path()?;
        if !path.exists() {
            let strategy = Self::default();
            strategy.save()?;
            log::info("strategy", "created default strategy.json");
            return Ok(strategy);
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let mut strategy: Self = serde_json::from_str(&data).context("parse strategy.json")?;
        if strategy.migrate() {
            if let Err(err) = strategy.save() {
                log::error("strategy", format!("migrate save failed: {err:#}"));
            } else {
                log::info("strategy", "migrated strategy schema");
            }
        }
        Ok(strategy)
    }

    pub fn save(&self) -> Result<()> {
        let path = paths::strategy_path()?;
        let tmp = path.with_extension("json.tmp");
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&tmp, data)?;
        fs::rename(&tmp, &path)?;
        log::debug("strategy", "saved strategy.json");
        Ok(())
    }

    pub fn add_subscription(&mut self, name: String, url: String) -> &Subscription {
        self.subscriptions.push(Subscription {
            id: Uuid::new_v4().to_string(),
            name,
            url,
        });
        self.subscriptions.last().expect("just pushed")
    }

    pub fn remove_subscription(&mut self, id_or_name: &str) -> bool {
        let before = self.subscriptions.len();
        self.subscriptions
            .retain(|s| s.id != id_or_name && s.name != id_or_name);
        self.subscriptions.len() != before
    }

    pub fn add_group(&mut self, group: Group) -> &Group {
        self.groups.push(group);
        self.groups.last().expect("just pushed")
    }

    fn migrate(&mut self) -> bool {
        if self.schema >= STRATEGY_SCHEMA {
            if self.rules.is_empty() {
                return false;
            }
            self.fold_legacy_rules();
            return true;
        }
        if self.schema < 2 {
            for group in &mut self.groups {
                group.migrate_legacy();
            }
        }
        if self.schema < 3 {
            self.fold_legacy_rules();
        }
        if self.schema < 5 {
            self.ensure_telegram_routing();
        }
        self.schema = STRATEGY_SCHEMA;
        true
    }

    fn ensure_telegram_routing(&mut self) -> bool {
        let mut changed = false;
        if !self.groups.iter().any(|g| g.name == TELEGRAM_GROUP) {
            let mut group = Group::all_nodes(TELEGRAM_GROUP.into(), "select".into());
            if let Some(base) = self
                .groups
                .iter()
                .find(|g| g.name.eq_ignore_ascii_case("default") || g.name == "PROXY")
                .or_else(|| self.groups.first())
            {
                group.sources = base.sources.clone();
            }
            self.groups.push(group);
            changed = true;
        }
        let index = self.rule_sets.iter().position(|s| {
            s.name.eq_ignore_ascii_case(TELEGRAM_GROUP)
                || s.matchers.iter().any(|m| {
                    m.kind == "app"
                        && (m.value.eq_ignore_ascii_case("Telegram")
                            || m.value == "ru.keepcoder.telegram")
                })
        });
        if let Some(index) = index {
            let set = &mut self.rule_sets[index];
            let keep_via = set.via.eq_ignore_ascii_case("direct")
                || set.via.eq_ignore_ascii_case("reject")
                || set.via == TELEGRAM_GROUP;
            if !keep_via {
                set.via = TELEGRAM_GROUP.into();
                changed = true;
            }
            for matcher in telegram_matchers() {
                if !set.matchers.iter().any(|m| m.same_as(&matcher)) {
                    set.matchers.push(matcher);
                    changed = true;
                }
            }
        } else {
            self.rule_sets.push(telegram_rule_set());
            changed = true;
        }
        changed
    }

    fn fold_legacy_rules(&mut self) {
        if self.rule_sets.is_empty() {
            self.rule_sets = self.rules.drain(..).map(RuleSet::from_legacy).collect();
        } else {
            self.rules.clear();
        }
    }

    pub fn group_mut(&mut self, name: &str) -> Option<&mut Group> {
        self.groups
            .iter_mut()
            .find(|g| g.name == name || g.id == name)
    }

    pub fn remove_group(&mut self, name: &str) -> bool {
        let before = self.groups.len();
        self.groups.retain(|g| g.name != name && g.id != name);
        self.groups.len() != before
    }

    pub fn update_group(&mut self, id: &str, mut next: Group) -> bool {
        let Some(group) = self.groups.iter_mut().find(|g| g.id == id) else {
            return false;
        };
        next.id = group.id.clone();
        *group = next;
        true
    }

    pub fn add_rule_set(&mut self, set: RuleSet) -> &RuleSet {
        self.rule_sets.push(set);
        self.rule_sets.last().expect("just pushed")
    }

    pub fn add_matcher(&mut self, name: String, matcher: Matcher, via: String) -> &RuleSet {
        if let Some(index) = self
            .rule_sets
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(&name))
        {
            let set = &mut self.rule_sets[index];
            if !set.matchers.iter().any(|m| m.same_as(&matcher)) {
                set.matchers.push(matcher);
            }
            if !via.trim().is_empty() {
                set.via = via;
            }
            return &self.rule_sets[index];
        }
        self.add_rule_set(RuleSet {
            id: Uuid::new_v4().to_string(),
            name,
            via,
            matchers: vec![matcher],
        })
    }

    pub fn update_rule_set(&mut self, id: &str, mut next: RuleSet) -> bool {
        let Some(set) = self.rule_sets.iter_mut().find(|s| s.id == id) else {
            return false;
        };
        next.id = set.id.clone();
        *set = next;
        true
    }

    pub fn set_rule_via(&mut self, id: &str, via: String) -> bool {
        let via = via.trim().to_string();
        if via.is_empty() {
            return false;
        }
        let Some(set) = self.rule_sets.iter_mut().find(|s| s.id == id) else {
            return false;
        };
        set.via = via;
        true
    }

    pub fn move_rule(&mut self, id: &str, delta: i32) -> bool {
        let Some(from) = self.rule_sets.iter().position(|s| s.id == id) else {
            return false;
        };
        let to = from as i32 + delta;
        if to < 0 || to >= self.rule_sets.len() as i32 {
            return false;
        }
        let item = self.rule_sets.remove(from);
        self.rule_sets.insert(to as usize, item);
        true
    }

    pub fn remove_rule(&mut self, id: &str) -> bool {
        let before = self.rule_sets.len();
        self.rule_sets.retain(|s| s.id != id && s.name != id);
        self.rule_sets.len() != before
    }
}

impl Group {
    pub fn all_nodes(name: String, kind: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            kind,
            all_nodes: true,
            sources: Vec::new(),
            name_contains: Vec::new(),
            name_excludes: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            filter: String::new(),
        }
    }

    pub fn matching(
        name: String,
        kind: String,
        sources: Vec<String>,
        name_contains: Vec<String>,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            kind,
            all_nodes: false,
            sources,
            name_contains,
            name_excludes: Vec::new(),
            include: Vec::new(),
            exclude: Vec::new(),
            filter: String::new(),
        }
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind.as_str() {
            "fallback" => "自动切换",
            "url-test" => "延迟最低",
            _ => "手动选择",
        }
    }

    pub fn kind_setting_label(&self) -> &'static str {
        match self.kind.as_str() {
            "fallback" => "自动切换（不可用则下一个）",
            "url-test" => "延迟最低",
            _ => "手动选择",
        }
    }

    pub fn parse_kind(kind: &str) -> Result<String> {
        match kind.trim() {
            "select" | "fallback" | "url-test" => Ok(kind.trim().to_string()),
            _ => anyhow::bail!("kind must be select, fallback, or url-test"),
        }
    }

    pub fn policy_label(&self) -> String {
        let mut parts = Vec::new();
        if self.all_nodes {
            if self.sources.is_empty() {
                parts.push("全部已导入节点".into());
            } else {
                parts.push(format!("全部 · {}", self.sources.join(" / ")));
            }
        } else {
            if !self.sources.is_empty() {
                parts.push(format!("来源 {}", self.sources.join(" / ")));
            }
            if !self.name_contains.is_empty() {
                parts.push(format!("名称含 {}", self.name_contains.join(" / ")));
            }
            if parts.is_empty() {
                parts.push("无自动匹配（仅钉住）".into());
            }
        }
        if !self.name_excludes.is_empty() {
            parts.push(format!("名称不含 {}", self.name_excludes.join(" / ")));
        }
        if !self.include.is_empty() {
            parts.push(format!("钉住 {}", self.include.len()));
        }
        if !self.exclude.is_empty() {
            parts.push(format!("排除 {}", self.exclude.len()));
        }
        parts.join(" · ")
    }

    fn migrate_legacy(&mut self) {
        if !self.filter.trim().is_empty() {
            self.all_nodes = false;
            if self.name_contains.is_empty() {
                self.name_contains = split_legacy_filter(&self.filter);
            }
        } else if self.name_contains.is_empty()
            && self.sources.is_empty()
            && self.include.is_empty()
        {
            self.all_nodes = true;
        }
        self.filter.clear();
    }
}

pub fn parse_list(raw: &str) -> Vec<String> {
    raw.split([',', '，', '/', '|'])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn join_list(parts: &[String]) -> String {
    parts.join(", ")
}

fn split_legacy_filter(raw: &str) -> Vec<String> {
    let trimmed = raw.trim().trim_start_matches("(?i)").trim_start_matches("(?-i)");
    trimmed
        .split('|')
        .map(|part| {
            part.trim()
                .trim_start_matches("(?i)")
                .trim_matches(|c: char| c == '(' || c == ')')
                .trim()
                .to_string()
        })
        .filter(|part| !part.is_empty() && part != "?i")
        .collect()
}

impl Rule {
    pub fn new_app(app: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app,
            domain: String::new(),
            suffix: String::new(),
            keyword: String::new(),
            via,
        }
    }

    pub fn new_domain(domain: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain,
            suffix: String::new(),
            keyword: String::new(),
            via,
        }
    }

    pub fn new_suffix(suffix: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain: String::new(),
            suffix,
            keyword: String::new(),
            via,
        }
    }

    pub fn new_keyword(keyword: String, via: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            app: String::new(),
            domain: String::new(),
            suffix: String::new(),
            keyword,
            via,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        if !self.app.is_empty() {
            "进程"
        } else if !self.keyword.is_empty() {
            "关键字"
        } else if !self.domain.is_empty() {
            "域名"
        } else if !self.suffix.is_empty() {
            "后缀"
        } else {
            "—"
        }
    }

    pub fn match_value(&self) -> &str {
        if !self.app.is_empty() {
            &self.app
        } else if !self.keyword.is_empty() {
            &self.keyword
        } else if !self.domain.is_empty() {
            &self.domain
        } else {
            self.suffix.trim_start_matches('.')
        }
    }

    pub fn match_label(&self) -> String {
        if !self.app.is_empty() {
            format!("app:{}", self.app)
        } else if !self.keyword.is_empty() {
            format!("keyword:{}", self.keyword)
        } else if !self.domain.is_empty() {
            format!("domain:{}", self.domain)
        } else if !self.suffix.is_empty() {
            format!("*.{}", self.suffix.trim_start_matches('.'))
        } else {
            "any".into()
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        let lower = q.to_lowercase();
        self.kind_label().contains(q)
            || self.match_value().to_lowercase().contains(&lower)
            || self.via.to_lowercase().contains(&lower)
            || self.match_label().to_lowercase().contains(&lower)
    }
}

impl Matcher {
    pub fn app(value: String) -> Self {
        Self {
            kind: "app".into(),
            value,
        }
    }

    pub fn domain(value: String) -> Self {
        Self {
            kind: "domain".into(),
            value,
        }
    }

    pub fn suffix(value: String) -> Self {
        Self {
            kind: "suffix".into(),
            value: value
                .trim()
                .trim_start_matches("*.")
                .trim_start_matches('.')
                .to_string(),
        }
    }

    pub fn keyword(value: String) -> Self {
        Self {
            kind: "keyword".into(),
            value,
        }
    }

    pub fn cidr(value: String) -> Self {
        Self {
            kind: "cidr".into(),
            value: value.trim().to_string(),
        }
    }

    pub fn from_rule(rule: &Rule) -> Self {
        if !rule.app.is_empty() {
            Self::app(rule.app.clone())
        } else if !rule.keyword.is_empty() {
            Self::keyword(rule.keyword.clone())
        } else if !rule.domain.is_empty() {
            Self::domain(rule.domain.clone())
        } else {
            Self::suffix(rule.suffix.clone())
        }
    }

    pub fn same_as(&self, other: &Self) -> bool {
        self.kind == other.kind && self.value.eq_ignore_ascii_case(&other.value)
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind.as_str() {
            "app" => "进程",
            "keyword" => "关键字",
            "domain" => "域名",
            "suffix" => "后缀",
            "cidr" => "网段",
            _ => "—",
        }
    }

    pub fn display_value(&self) -> String {
        if self.kind == "suffix" {
            format!("*.{}", self.value.trim_start_matches('.'))
        } else {
            self.value.clone()
        }
    }
}

impl RuleSet {
    pub fn from_legacy(rule: Rule) -> Self {
        let name = rule.match_value().to_string();
        let matcher = Matcher::from_rule(&rule);
        Self {
            id: rule.id,
            name,
            via: rule.via,
            matchers: vec![matcher],
        }
    }

    pub fn matches_query(&self, query: &str) -> bool {
        let q = query.trim();
        if q.is_empty() {
            return true;
        }
        let lower = q.to_lowercase();
        self.name.to_lowercase().contains(&lower)
            || self.via.to_lowercase().contains(&lower)
            || self.matchers.iter().any(|m| {
                m.kind_label().contains(q)
                    || m.value.to_lowercase().contains(&lower)
                    || m.display_value().to_lowercase().contains(&lower)
            })
    }
}

pub fn load_from(path: &Path) -> Result<Strategy> {
    let data = fs::read_to_string(path)?;
    let mut strategy: Strategy = serde_json::from_str(&data)?;
    strategy.migrate();
    Ok(strategy)
}
