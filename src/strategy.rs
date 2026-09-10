use std::collections::HashSet;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::log;
use crate::paths;

pub const DEFAULT_EXCLUDE: &str = r"(?i)(流量|剩余|到期|官网|重置|过期|剩余流量|套餐到期|过期时间)";

pub const DEFAULT_MIXED_PORT: u16 = 7890;

pub const STRATEGY_SCHEMA: u32 = 7;

pub const TELEGRAM_GROUP: &str = "Telegram";

/// Mihomo's built-in selector. Mixed/SE「全局」pins inbound to this name.
pub const GLOBAL_GROUP: &str = "GLOBAL";

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

/// Per-inbound routing mode. Mixed and System Extension each have their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum InboundMode {
    #[default]
    Rule,
    Proxy,
    Global,
    Direct,
}

impl InboundMode {
    pub const ALL: [Self; 4] = [Self::Rule, Self::Proxy, Self::Global, Self::Direct];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rule => "rule",
            Self::Proxy => "proxy",
            Self::Global => "global",
            Self::Direct => "direct",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rule => "按规则",
            Self::Proxy => "默认组",
            Self::Global => "全局",
            Self::Direct => "直连",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "rule" => Ok(Self::Rule),
            "proxy" => Ok(Self::Proxy),
            "global" => Ok(Self::Global),
            "direct" => Ok(Self::Direct),
            _ => anyhow::bail!("mode must be rule, proxy, global, or direct"),
        }
    }
}

/// Fallback policy after user rules. GFWList is a whole-set install, not matchers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoutingProfile {
    #[default]
    Allowlist,
    Gfwlist,
    Group,
    Chinadirect,
}

impl RoutingProfile {
    pub const ALL: [Self; 4] = [Self::Allowlist, Self::Gfwlist, Self::Group, Self::Chinadirect];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allowlist => "allowlist",
            Self::Gfwlist => "gfwlist",
            Self::Group => "group",
            Self::Chinadirect => "chinadirect",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Allowlist => "未命中直连",
            Self::Gfwlist => "GFWList",
            Self::Group => "未匹配走组",
            Self::Chinadirect => "国内直连",
        }
    }

    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "allowlist" | "direct" => Ok(Self::Allowlist),
            "gfwlist" | "gfw" => Ok(Self::Gfwlist),
            "group" => Ok(Self::Group),
            "chinadirect" | "china" | "geoip-cn" | "geoipcn" => Ok(Self::Chinadirect),
            _ => anyhow::bail!("routing must be allowlist, gfwlist, group, or chinadirect"),
        }
    }
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
    #[serde(default = "default_mixed_port")]
    pub mixed_port: u16,
    pub exclude_filter: String,
    /// When true, myproxy writes debug traces to `myproxy.log` and the Settings page.
    #[serde(default)]
    pub developer_mode: bool,
    /// None follows the installed build's default until a channel is selected.
    #[serde(default)]
    pub update_channel: Option<crate::updates::UpdateChannel>,
    /// When true, compile mihomo TUN so apps need not point HTTP/SOCKS at Mixed.
    #[serde(default)]
    pub tun: bool,
    /// When true, activate MClash-style System Extension intercept (NETransparentProxyProvider).
    #[serde(default)]
    pub system_extension: bool,
    /// When true, RFC1918 destinations skip the built-in private-network bypass.
    #[serde(default)]
    pub lan_capture: bool,
    /// When true, point macOS system HTTP/HTTPS/SOCKS at Mixed while connected.
    #[serde(default)]
    pub system_proxy: bool,
    /// How Mixed inbound traffic is routed. Independent of `extension_mode`.
    #[serde(default)]
    pub mixed_mode: InboundMode,
    /// How System Extension inbound traffic is routed. Independent of `mixed_mode`.
    #[serde(default)]
    pub extension_mode: InboundMode,
    /// Current member of mihomo's built-in GLOBAL selector. Restored on apply.
    #[serde(default)]
    pub global_selected: String,
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
    /// Unmatched traffic (`MATCH`). Empty and `DIRECT` are whitelist
    /// (positive list only). Any other value is a group name.
    #[serde(default)]
    pub routing_profile: RoutingProfile,
    #[serde(default)]
    pub unmatched_via: String,
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
    /// Last user-picked member for select groups. Restored after connect/reload.
    #[serde(default)]
    pub selected: String,
    /// Legacy single regex. Read on migrate, never written back.
    #[serde(default, skip_serializing)]
    pub filter: String,
}

fn default_mixed_port() -> u16 {
    DEFAULT_MIXED_PORT
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
            mixed_port: DEFAULT_MIXED_PORT,
            exclude_filter: DEFAULT_EXCLUDE.into(),
            developer_mode: false,
            update_channel: None,
            tun: false,
            system_extension: false,
            lan_capture: false,
            system_proxy: false,
            mixed_mode: InboundMode::Rule,
            extension_mode: InboundMode::Rule,
            global_selected: String::new(),
            launch_at_login: false,
            silent_launch: false,
            lite_mode: false,
            connect_on_launch: false,
            routing_profile: RoutingProfile::Allowlist,
            unmatched_via: "DIRECT".into(),
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
        let data = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
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
        self.validate()?;
        let path = paths::strategy_path()?;
        let data = serde_json::to_string_pretty(self)?;
        paths::atomic_write(&path, data.as_bytes())?;
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

    /// Validate persisted intent without fetching subscriptions or touching the running core.
    pub fn validate(&self) -> Result<()> {
        let controller = self
            .mixed_port
            .checked_add(107)
            .filter(|_| self.mixed_port != 0)
            .context("mixed port must be between 1 and 65428")?;
        if self.tun && self.system_extension {
            anyhow::bail!("TUN and System Extension cannot both be enabled");
        }
        if self.tun || self.system_extension {
            let dns = crate::compile::DNS_LISTEN_PORT;
            if self.mixed_port == dns
                || controller == dns
                || (self.system_extension && self.mixed_port + 1 == dns)
            {
                anyhow::bail!(
                    "mixed/controller/System Extension listener conflicts with DNS port {dns}"
                );
            }
        }
        regex::Regex::new(&self.exclude_filter).context("invalid exclude_filter regex")?;
        let mut names = HashSet::new();
        let mut ids = HashSet::new();
        for group in &self.groups {
            validate_name("group", &group.name)?;
            if reserved_proxy_name(&group.name)
                || ["group:", "node:", "gfw:", "gfwlist:"]
                    .iter()
                    .any(|prefix| group.name.to_ascii_lowercase().starts_with(prefix))
            {
                anyhow::bail!("reserved group name: {}", group.name);
            }
            if !names.insert(group.name.to_ascii_lowercase()) {
                anyhow::bail!("duplicate group name: {}", group.name);
            }
            if group.id.is_empty() || !ids.insert(&group.id) {
                anyhow::bail!("duplicate or empty group id: {}", group.name);
            }
            Group::parse_kind(&group.kind).with_context(|| format!("group {}", group.name))?;
        }
        names.clear();
        ids.clear();
        for subscription in &self.subscriptions {
            validate_name("subscription", &subscription.name)?;
            if !names.insert(subscription.name.to_ascii_lowercase()) {
                anyhow::bail!("duplicate subscription name: {}", subscription.name);
            }
            if subscription.id.is_empty() || !ids.insert(&subscription.id) {
                anyhow::bail!("duplicate or empty subscription id: {}", subscription.name);
            }
            if subscription.url.trim().is_empty() {
                anyhow::bail!("subscription {} has an empty URL", subscription.name);
            }
        }
        names.clear();
        ids.clear();
        for set in &self.rule_sets {
            validate_name("rule", &set.name)?;
            if !names.insert(set.name.to_ascii_lowercase()) {
                anyhow::bail!(
                    "duplicate rule name: {}; edit the existing rule instead",
                    set.name
                );
            }
            if set.id.is_empty() || !ids.insert(&set.id) {
                anyhow::bail!("duplicate or empty rule id: {}", set.name);
            }
            if set.matchers.is_empty() {
                anyhow::bail!("rule {} needs at least one matcher", set.name);
            }
            self.validate_target(&set.via, None)
                .with_context(|| format!("rule {} target", set.name))?;
            for matcher in &set.matchers {
                matcher
                    .validate()
                    .with_context(|| format!("rule {}", set.name))?;
                if crate::gfw::gfw_group(&set.via).is_some() && matcher.kind == "cidr" {
                    anyhow::bail!(
                        "rule {}: gfw targets cannot evaluate a CIDR against a domain list",
                        set.name
                    );
                }
            }
        }
        if matches!(
            self.routing_profile,
            RoutingProfile::Group | RoutingProfile::Chinadirect
        ) {
            self.validate_target(&self.unmatched_via, None)
                .context("unmatched target")?;
        }
        if !self.global_selected.is_empty() {
            validate_name("GLOBAL selection", &self.global_selected)?;
        }
        if self.groups.is_empty()
            && (self.mixed_mode == InboundMode::Proxy
                || (self.system_extension && self.extension_mode == InboundMode::Proxy)
                || self.routing_profile == RoutingProfile::Gfwlist
                || self.routing_profile == RoutingProfile::Chinadirect)
        {
            anyhow::bail!("the selected inbound/routing mode requires a proxy group");
        }
        Ok(())
    }

    /// Candidate validation uses the catalog that will actually be applied.
    /// Saved intent may refer to nodes from subscriptions that have not refreshed yet.
    pub fn validate_catalog(&self, catalog: &crate::catalog::Catalog) -> Result<()> {
        catalog.validate()?;
        for node in &catalog.nodes {
            if self
                .groups
                .iter()
                .any(|group| group.name.eq_ignore_ascii_case(&node.name))
            {
                anyhow::bail!("node name conflicts with a group: {}", node.name);
            }
        }
        for set in &self.rule_sets {
            self.validate_target(&set.via, Some(catalog))
                .with_context(|| format!("rule {} target", set.name))?;
        }
        if matches!(
            self.routing_profile,
            RoutingProfile::Group | RoutingProfile::Chinadirect
        ) {
            self.validate_target(&self.unmatched_via, Some(catalog))
                .context("unmatched target")?;
        }
        // Historical selector picks are checked against live members when restored;
        // a removed subscription node must not prevent a safe default from loading.
        Ok(())
    }

    fn validate_target(&self, raw: &str, catalog: Option<&crate::catalog::Catalog>) -> Result<()> {
        validate_name("target", raw)?;
        let gfw = crate::gfw::gfw_group(raw);
        let target = gfw.unwrap_or(raw);
        let explicit_group = target.strip_prefix("group:");
        let explicit_node = target.strip_prefix("node:");
        if gfw.is_some() && explicit_node.is_some() {
            anyhow::bail!("gfw target must name a group");
        }
        let target = explicit_group.or(explicit_node).unwrap_or(target);
        if target.is_empty() {
            anyhow::bail!("target name cannot be empty");
        }
        if explicit_node.is_none() {
            let group = self.resolve_group_reference(target);
            if group.is_some() {
                return Ok(());
            }
            if gfw.is_some() || explicit_group.is_some() {
                anyhow::bail!("group does not exist: {target}");
            }
            if target.eq_ignore_ascii_case("DIRECT") || target.eq_ignore_ascii_case("REJECT") {
                return Ok(());
            }
        }
        if let Some(catalog) = catalog {
            if !catalog.nodes.iter().any(|node| node.name == target) {
                anyhow::bail!("target does not exist: {target}");
            }
        } else if target.eq_ignore_ascii_case(GLOBAL_GROUP) || target.ends_with(':') {
            anyhow::bail!("invalid target: {target}");
        }
        Ok(())
    }

    fn resolve_group_reference(&self, name: &str) -> Option<&Group> {
        self.groups
            .iter()
            .find(|group| group.name.eq_ignore_ascii_case(name))
            .or_else(|| {
                if name.eq_ignore_ascii_case("default") || name.eq_ignore_ascii_case("proxy") {
                    self.groups
                        .iter()
                        .find(|group| group.name == self.default_group_name())
                } else {
                    None
                }
            })
    }

    fn references_group(&self, raw: &str, id: &str) -> bool {
        if raw.starts_with("node:") {
            return false;
        }
        let name = crate::gfw::gfw_group(raw).unwrap_or(raw);
        let name = name.strip_prefix("group:").unwrap_or(name);
        self.resolve_group_reference(name)
            .is_some_and(|group| group.id == id)
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
        if self.schema < 7 {
            self.migrate_routing_profile();
        }
        self.schema = STRATEGY_SCHEMA;
        true
    }

    fn migrate_routing_profile(&mut self) {
        let via = self.unmatched_via.trim();
        self.routing_profile = if via.is_empty() || via.eq_ignore_ascii_case("direct") {
            RoutingProfile::Allowlist
        } else {
            RoutingProfile::Group
        };
    }

    pub fn set_routing_profile(&mut self, profile: RoutingProfile) {
        self.routing_profile = profile;
        if matches!(
            profile,
            RoutingProfile::Group | RoutingProfile::Chinadirect
        ) {
            let via = self.unmatched_via.trim();
            if via.is_empty() || via.eq_ignore_ascii_case("direct") {
                self.unmatched_via = self.default_group_name().to_string();
            }
        }
    }

    pub fn uses_global(&self) -> bool {
        self.mixed_mode == InboundMode::Global || self.extension_mode == InboundMode::Global
    }

    /// Empty GLOBAL pick becomes the default group so「全局」is not silently DIRECT.
    pub fn ensure_global_selected(&mut self) -> bool {
        if !self.global_selected.trim().is_empty() {
            return false;
        }
        self.global_selected = self.default_group_name().to_string();
        true
    }

    pub fn set_global_selected(&mut self, node: String) -> bool {
        let next = node.trim();
        if next.is_empty() {
            return false;
        }
        if self.global_selected == next {
            return true;
        }
        self.global_selected = next.to_string();
        true
    }

    pub fn default_group_name(&self) -> &str {
        self.groups
            .iter()
            .find(|group| group.name == "PROXY" || group.name.eq_ignore_ascii_case("default"))
            .or_else(|| self.groups.first())
            .map(|group| group.name.as_str())
            .unwrap_or("DIRECT")
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
                || set.via == TELEGRAM_GROUP
                || crate::gfw::gfw_group(&set.via).is_some();
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
        self.remove_group_checked(name).is_ok()
    }

    pub fn remove_group_checked(&mut self, name: &str) -> Result<()> {
        let group = self
            .groups
            .iter()
            .find(|group| group.name == name || group.id == name)
            .with_context(|| format!("group not found: {name}"))?;
        let mut refs = Vec::new();
        for set in &self.rule_sets {
            if self.references_group(&set.via, &group.id) {
                refs.push(format!("rule {}", set.name));
            }
        }
        if self.references_group(&self.unmatched_via, &group.id) {
            refs.push("unmatched_via".into());
        }
        if self.references_group(&self.global_selected, &group.id) {
            refs.push("GLOBAL".into());
        }
        if group.name == self.default_group_name()
            && (self.mixed_mode == InboundMode::Proxy
                || (self.system_extension && self.extension_mode == InboundMode::Proxy)
                || self.routing_profile == RoutingProfile::Gfwlist)
        {
            refs.push("default inbound/routing group".into());
        }
        if !refs.is_empty() {
            anyhow::bail!(
                "group {} is still referenced by {}; change those targets first",
                group.name,
                refs.join(", ")
            );
        }
        let id = group.id.clone();
        self.groups.retain(|group| group.id != id);
        Ok(())
    }

    pub fn update_group(&mut self, id: &str, mut next: Group) -> Result<()> {
        let index = self
            .groups
            .iter()
            .position(|group| group.id == id)
            .context("节点组已不存在")?;
        let renamed = self.groups[index].name != next.name;
        if renamed {
            let mut candidate = self.clone();
            candidate.groups[index].name = next.name.clone();
            let previous_default = self
                .groups
                .iter()
                .find(|group| group.name == self.default_group_name());
            let next_default = candidate
                .groups
                .iter()
                .find(|group| group.name == candidate.default_group_name());
            if previous_default.map(|group| &group.id) != next_default.map(|group| &group.id)
                && (self.mixed_mode == InboundMode::Proxy
                    || (self.system_extension && self.extension_mode == InboundMode::Proxy)
                    || self.routing_profile == RoutingProfile::Gfwlist)
            {
                anyhow::bail!("改名会改变正在使用的默认节点组；请先调整入口模式或分流预设");
            }
            // Resolve aliases before replacement, while the old default group still exists.
            let refs: Vec<bool> = self
                .rule_sets
                .iter()
                .map(|set| self.references_group(&set.via, id))
                .collect();
            let fallback = self.references_group(&self.unmatched_via, id);
            let global = self.references_group(&self.global_selected, id);
            for (set, referenced) in self.rule_sets.iter_mut().zip(refs) {
                if referenced {
                    rename_target(&mut set.via, &next.name);
                }
            }
            if fallback {
                rename_target(&mut self.unmatched_via, &next.name);
            }
            if global {
                rename_target(&mut self.global_selected, &next.name);
            }
        }
        next.id = self.groups[index].id.clone();
        if next.selected.trim().is_empty() {
            next.selected = self.groups[index].selected.clone();
        }
        self.groups[index] = next;
        Ok(())
    }

    pub fn set_group_selected(&mut self, id: &str, node: String) -> bool {
        let Some(group) = self.groups.iter_mut().find(|g| g.id == id || g.name == id) else {
            return false;
        };
        if group.kind != "select" {
            return false;
        }
        group.selected = node;
        true
    }

    pub fn add_rule_set(&mut self, set: RuleSet) -> &RuleSet {
        self.rule_sets.push(set);
        self.rule_sets.last().expect("just pushed")
    }

    pub fn add_matcher(&mut self, name: String, matcher: Matcher, via: String) -> Result<&RuleSet> {
        if self
            .rule_sets
            .iter()
            .any(|set| set.name.eq_ignore_ascii_case(&name))
        {
            anyhow::bail!("duplicate rule name: {name}; edit the existing rule instead");
        }
        matcher.validate()?;
        Ok(self.add_rule_set(RuleSet {
            id: Uuid::new_v4().to_string(),
            name,
            via,
            matchers: vec![matcher],
        }))
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
            selected: String::new(),
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
            selected: String::new(),
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
            if self.name_contains.is_empty() {
                parts.push("无自动匹配（仅钉住；来源只限定范围）".into());
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

fn validate_name(kind: &str, name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name != name.trim()
        || name.chars().any(|ch| ch == ',' || ch.is_control())
    {
        anyhow::bail!("invalid {kind} name/value: use non-empty text without surrounding whitespace, commas or control characters");
    }
    Ok(())
}

pub(crate) fn reserved_proxy_name(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "DIRECT" | "REJECT" | "GLOBAL" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
    )
}

fn rename_target(target: &mut String, name: &str) {
    if crate::gfw::gfw_group(target).is_some() {
        let prefix = target
            .split_once(':')
            .map(|(prefix, _)| prefix)
            .unwrap_or("gfw");
        *target = format!("{prefix}:{name}");
    } else if target.starts_with("group:") {
        *target = format!("group:{name}");
    } else {
        *target = name.to_string();
    }
}

pub fn parse_list(raw: &str) -> Vec<String> {
    raw.split([',', '，', '/', '|'])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn parse_matcher_list(raw: &str) -> Vec<String> {
    raw.split([',', '，', '|', '\n'])
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect()
}

fn validate_cidr(raw: &str) -> Result<()> {
    let (ip, prefix) = raw
        .trim()
        .split_once('/')
        .with_context(|| format!("invalid CIDR: {raw}"))?;
    let ip: IpAddr = ip
        .parse()
        .with_context(|| format!("invalid CIDR address: {raw}"))?;
    let prefix: u8 = prefix
        .parse()
        .with_context(|| format!("invalid CIDR prefix: {raw}"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if prefix > max {
        anyhow::bail!("CIDR prefix out of range: {raw}");
    }
    Ok(())
}

pub fn join_list(parts: &[String]) -> String {
    parts.join(", ")
}

fn split_legacy_filter(raw: &str) -> Vec<String> {
    let trimmed = raw
        .trim()
        .trim_start_matches("(?i)")
        .trim_start_matches("(?-i)");
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
    pub fn validate(&self) -> Result<()> {
        validate_name("matcher", &self.value)?;
        match self.kind.as_str() {
            "cidr" => validate_cidr(&self.value),
            "app" | "domain" | "suffix" | "keyword" => Ok(()),
            _ => anyhow::bail!("unsupported matcher kind: {}", self.kind),
        }
    }

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

/// Pretty-print `strategy` to `path`. Does not touch the live strategy file.
pub fn export_to(strategy: &Strategy, path: &Path) -> Result<()> {
    strategy.validate().context("invalid strategy")?;
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let data = serde_json::to_string_pretty(strategy)?;
    paths::atomic_write(path, data.as_bytes())
        .with_context(|| format!("write {}", path.display()))?;
    log::info(
        "strategy",
        format!("exported strategy to {}", path.display()),
    );
    Ok(())
}

/// Parse, migrate, and validate a strategy file without writing the live copy.
pub fn parse_import(path: &Path) -> Result<Strategy> {
    let data = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    if data.trim().is_empty() {
        anyhow::bail!("strategy file is empty");
    }
    let value: serde_json::Value = serde_json::from_str(&data).context("parse strategy JSON")?;
    if !value.is_object() {
        anyhow::bail!("strategy file must be a JSON object");
    }
    let mut strategy: Strategy = serde_json::from_value(value).context("decode strategy JSON")?;
    strategy.migrate();
    strategy.validate().context("invalid strategy")?;
    Ok(strategy)
}

#[derive(Debug, Clone)]
pub struct ImportOutcome {
    pub strategy: Strategy,
    pub backup: Option<PathBuf>,
}

/// Replace the live `strategy.json` after writing `strategy.json.bak-import-*`.
pub fn import_from(path: &Path) -> Result<ImportOutcome> {
    let strategy = parse_import(path)?;
    let current = paths::strategy_path()?;
    let backup = backup_strategy_at(&current)?;
    strategy.save()?;
    log::info(
        "strategy",
        format!(
            "imported strategy subscriptions={} groups={} rules={}",
            strategy.subscriptions.len(),
            strategy.groups.len(),
            strategy.rule_sets.len()
        ),
    );
    Ok(ImportOutcome { strategy, backup })
}

pub fn default_export_name() -> String {
    format!("myproxy-strategy-{}.json", local_date_stamp())
}

pub fn default_export_path() -> Result<PathBuf> {
    let dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .context("no Downloads directory")?;
    Ok(dir.join(default_export_name()))
}

fn backup_strategy_at(current: &Path) -> Result<Option<PathBuf>> {
    if !current.exists() {
        return Ok(None);
    }
    let backup = import_backup_path(current);
    fs::copy(current, &backup).with_context(|| format!("backup {}", current.display()))?;
    log::info("strategy", format!("import backup {}", backup.display()));
    Ok(Some(backup))
}

fn import_backup_path(current: &Path) -> PathBuf {
    let stamp = local_datetime_stamp();
    let candidate = current.with_file_name(format!("strategy.json.bak-import-{stamp}"));
    if !candidate.exists() {
        return candidate;
    }
    current.with_file_name(format!(
        "strategy.json.bak-import-{stamp}-{}",
        &Uuid::new_v4().simple().to_string()[..6]
    ))
}

fn local_date_stamp() -> String {
    let tm = local_now();
    format!(
        "{:04}-{:02}-{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday
    )
}

fn local_datetime_stamp() -> String {
    let tm = local_now();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

fn local_now() -> libc::tm {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm = std::mem::zeroed::<libc::tm>();
        libc::localtime_r(&now, &mut tm);
        tm
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_inbound_modes_default_to_rule() {
        let json = r#"{
            "schema": 5,
            "exclude_filter": "",
            "subscriptions": [],
            "groups": [],
            "rule_sets": []
        }"#;
        let mut strategy: Strategy = serde_json::from_str(json).expect("parse schema 5");
        assert_eq!(strategy.mixed_mode, InboundMode::Rule);
        assert_eq!(strategy.extension_mode, InboundMode::Rule);
        assert!(strategy.migrate());
        assert_eq!(strategy.schema, STRATEGY_SCHEMA);
        assert_eq!(strategy.mixed_mode, InboundMode::Rule);
        assert_eq!(strategy.extension_mode, InboundMode::Rule);
        assert_eq!(strategy.routing_profile, RoutingProfile::Allowlist);
        assert_eq!(strategy.routing_profile.label(), "未命中直连");
    }

    #[test]
    fn missing_routing_profile_defaults_to_allowlist() {
        let json = r#"{
            "schema": 6,
            "exclude_filter": "",
            "subscriptions": [],
            "groups": [],
            "rule_sets": []
        }"#;
        let mut strategy: Strategy = serde_json::from_str(json).expect("parse schema 6");
        assert_eq!(strategy.routing_profile, RoutingProfile::Allowlist);
        assert!(strategy.migrate());
        assert_eq!(strategy.schema, STRATEGY_SCHEMA);
        assert_eq!(strategy.routing_profile, RoutingProfile::Allowlist);
    }

    #[test]
    fn unmatched_group_migrates_to_group_profile() {
        let json = r#"{
            "schema": 6,
            "exclude_filter": "",
            "unmatched_via": "PROXY",
            "subscriptions": [],
            "groups": [],
            "rule_sets": []
        }"#;
        let mut strategy: Strategy = serde_json::from_str(json).expect("parse schema 6");
        assert!(strategy.migrate());
        assert_eq!(strategy.schema, STRATEGY_SCHEMA);
        assert_eq!(strategy.routing_profile, RoutingProfile::Group);
        assert_eq!(strategy.unmatched_via, "PROXY");
    }

    #[test]
    fn inbound_mode_roundtrips_json() {
        let mut strategy = Strategy::default();
        strategy.mixed_mode = InboundMode::Global;
        strategy.extension_mode = InboundMode::Direct;
        let json = serde_json::to_string(&strategy).expect("serialize");
        let parsed: Strategy = serde_json::from_str(&json).expect("parse");
        assert_eq!(parsed.mixed_mode, InboundMode::Global);
        assert_eq!(parsed.extension_mode, InboundMode::Direct);
        assert!(json.contains("\"mixed_mode\":\"global\""));
        assert!(json.contains("\"extension_mode\":\"direct\""));
    }

    #[test]
    fn missing_global_selected_defaults_empty() {
        let json = r#"{
            "schema": 7,
            "exclude_filter": "",
            "subscriptions": [],
            "groups": [],
            "rule_sets": []
        }"#;
        let strategy: Strategy = serde_json::from_str(json).expect("parse");
        assert!(strategy.global_selected.is_empty());
        assert!(!strategy.uses_global());
    }

    #[test]
    fn ensure_global_selected_uses_default_group() {
        let mut strategy = Strategy::default();
        assert!(strategy.ensure_global_selected());
        assert_eq!(strategy.global_selected, "PROXY");
        assert!(!strategy.ensure_global_selected());
        assert!(strategy.set_global_selected("DIRECT".into()));
        assert_eq!(strategy.global_selected, "DIRECT");
        assert!(!strategy.set_global_selected("  ".into()));
    }

    fn temp_json(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("myproxy-strategy-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir.join(name)
    }

    #[test]
    fn export_to_roundtrips_through_load_from() {
        let mut strategy = Strategy::default();
        strategy.mixed_port = 7891;
        strategy.developer_mode = true;
        let path = temp_json("strategy.json");
        export_to(&strategy, &path).expect("export");
        let loaded = load_from(&path).expect("load export");
        assert_eq!(loaded, strategy);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn parse_import_migrates_schema_5() {
        let path = temp_json("old.json");
        fs::write(
            &path,
            r#"{
            "schema": 5,
            "exclude_filter": "",
            "subscriptions": [],
            "groups": [],
            "rule_sets": []
        }"#,
        )
        .expect("write");
        let imported = parse_import(&path).expect("parse schema 5");
        assert_eq!(imported.schema, STRATEGY_SCHEMA);
        assert_eq!(imported.mixed_mode, InboundMode::Rule);
        assert_eq!(imported.routing_profile, RoutingProfile::Allowlist);
        let _ = fs::remove_dir_all(path.parent().expect("parent"));
    }

    #[test]
    fn parse_import_rejects_empty_and_non_object() {
        let empty = temp_json("empty.json");
        fs::write(&empty, "   ").expect("write empty");
        assert!(parse_import(&empty).is_err());

        let array = temp_json("array.json");
        fs::write(&array, "[]").expect("write array");
        assert!(parse_import(&array).is_err());

        let invalid = temp_json("bad.json");
        fs::write(&invalid, "{not json").expect("write bad");
        assert!(parse_import(&invalid).is_err());
        let _ = fs::remove_dir_all(empty.parent().expect("parent"));
        let _ = fs::remove_dir_all(array.parent().expect("parent"));
        let _ = fs::remove_dir_all(invalid.parent().expect("parent"));
    }

    #[test]
    fn backup_strategy_keeps_previous_bytes() {
        let dir = std::env::temp_dir().join(format!("myproxy-bak-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        let current = dir.join("strategy.json");
        fs::write(&current, "{\"schema\":7}").expect("write current");
        let backup = backup_strategy_at(&current)
            .expect("backup")
            .expect("backup path");
        assert!(backup
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("strategy.json.bak-import-")));
        assert_eq!(
            fs::read_to_string(&backup).expect("read backup"),
            "{\"schema\":7}"
        );
        assert_eq!(
            fs::read_to_string(&current).expect("read current"),
            "{\"schema\":7}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_import_failure_leaves_existing_file() {
        let dir = std::env::temp_dir().join(format!("myproxy-keep-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        let current = dir.join("strategy.json");
        fs::write(&current, "keep-me").expect("write current");
        let bad = dir.join("bad.json");
        fs::write(&bad, "[]").expect("write bad");
        assert!(parse_import(&bad).is_err());
        assert_eq!(
            fs::read_to_string(&current).expect("read current"),
            "keep-me"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
