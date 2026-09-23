//! Shared policy and catalog implementation used by desktop and mobile.
//! The source modules are intentionally the existing desktop modules: this
//! crate is a platform-free facade, not a second matcher implementation.

pub mod log { pub fn debug(_: &str, _: String) {} pub fn info(_: &str, _: impl Into<String>) {} pub fn warn(_: &str, _: impl Into<String>) {} pub fn error(_: &str, _: impl Into<String>) {} }
pub mod paths { use std::path::PathBuf; pub fn data_dir() -> anyhow::Result<PathBuf> { anyhow::bail!("mobile core has no implicit filesystem data directory") } pub fn catalog_path() -> anyhow::Result<PathBuf> { anyhow::bail!("mobile core persistence belongs to the native caller") } pub fn strategy_path() -> anyhow::Result<PathBuf> { anyhow::bail!("mobile core persistence belongs to the native caller") } pub fn atomic_write(_: &std::path::Path, _: &[u8]) -> anyhow::Result<()> { anyhow::bail!("mobile core persistence belongs to the native caller") } }
pub mod backend { pub fn is_xray() -> bool { true } }
pub mod updates { #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)] #[serde(rename_all="lowercase")] pub enum UpdateChannel { Prod, Nightly, Xray } }
pub mod controller { #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)] pub struct LiveMember { pub name: String, pub delay: Option<u32> } #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)] pub struct LiveGroup { pub name: String, pub kind: String, pub now: String, pub members: Vec<LiveMember> } }
pub mod gfw { pub fn gfw_group(value: &str) -> Option<&str> { value.strip_prefix("gfw:").or_else(|| value.strip_prefix("gfwlist:")) } }
pub mod compile { pub const DNS_LISTEN_PORT: u16 = 1053; pub fn default_group(strategy: &crate::strategy::Strategy) -> &str { strategy.default_group_name() } }
pub mod xray {
    pub mod import { include!("../../../src/xray/import.rs"); }
    pub mod nodes { include!("../../../src/xray/nodes.rs"); }
    pub mod policy { include!("../../../src/xray/policy.rs"); }
    pub mod geo { include!("../../../src/xray/geo.rs"); }
    pub fn default_strategy() -> crate::strategy::Strategy {
        use crate::strategy::{Group, InboundMode, RoutingProfile, Strategy};
        let mut strategy = Strategy::default();
        strategy.mixed_port = 40808;
        strategy.mixed_mode = InboundMode::Global;
        strategy.global_selected = "节点选择".into();
        strategy.routing_profile = RoutingProfile::Group;
        strategy.unmatched_via = "节点选择".into();
        strategy.rule_sets.clear();
        let mut selector = Group::all_nodes("节点选择".into(), "select".into());
        selector.group_refs = vec!["美国优先".into(), "日本优先".into(), "香港优先".into()];
        strategy.groups = vec![selector];
        for (name, order) in [("美国优先", ["美国", "日本", "香港"]), ("日本优先", ["日本", "香港", "美国"]), ("香港优先", ["香港", "美国", "日本"])] {
            let mut profile = Group::matching(name.into(), "fallback".into(), vec![], vec![]);
            profile.group_refs = order.into_iter().map(str::to_owned).collect();
            strategy.groups.push(profile);
        }
        for (name, patterns) in [("美国", vec!["美国", "*|us|*", "United States", "🇺🇸"]), ("日本", vec!["日本", "*|jp|*", "Japan", "🇯🇵"]), ("香港", vec!["香港", "*|hk|*", "Hong Kong", "🇭🇰"])] {
            strategy.groups.push(Group::matching(name.into(), "url-test".into(), vec![], patterns.into_iter().map(str::to_owned).collect()));
        }
        strategy
    }
}
#[path = "../../../src/strategy.rs"] pub mod strategy;
#[path = "../../../src/catalog.rs"] pub mod catalog;
pub use strategy::{Group, InboundMode, Matcher, RoutingProfile, Rule, RuleSet, Strategy, Subscription, GLOBAL_GROUP};
pub use catalog::{Catalog, Node};
pub use xray::policy::{Decision, NodeHealth, Route};

pub fn parse_links(text: &str) -> anyhow::Result<Vec<serde_yaml::Value>> { xray::import::parse_links(text) }
pub fn render_node(node: &Node, tag: &str) -> anyhow::Result<serde_json::Value> { xray::nodes::render(node, tag) }
pub fn decide(strategy: &Strategy, catalog: &Catalog, health: &std::collections::HashMap<String, NodeHealth>, host: &str, port: u16, network: &str) -> Decision { xray::policy::decide_network(strategy, catalog, health, host, port, None, network) }
