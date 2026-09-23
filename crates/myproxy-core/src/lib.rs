//! Desktop and mobile compile the same policy source files. Persistence is
//! supplied by the native host; mobile never uses the desktop file locations.

#[path = "../../../src/strategy.rs"]
pub mod strategy;
#[path = "../../../src/catalog.rs"]
pub mod catalog;
#[path = "../../../src/xray/import.rs"]
pub mod import;
#[path = "../../../src/xray/nodes.rs"]
pub mod nodes;
#[path = "../../../src/xray/policy.rs"]
pub mod policy;
#[path = "../../../src/xray/geo.rs"]
pub mod geo;
#[path = "../../../src/xray/defaults.rs"]
pub mod defaults;

pub mod xray {
    pub use crate::{geo, import, nodes, policy};
    pub use crate::defaults::default_strategy;
}

pub mod paths {
    use std::path::{Path, PathBuf};
    pub fn data_dir() -> anyhow::Result<PathBuf> { anyhow::bail!("手机端尚未配置规则数据库") }
    pub fn catalog_path() -> anyhow::Result<PathBuf> { anyhow::bail!("手机配置由原生存储管理") }
    pub fn strategy_path() -> anyhow::Result<PathBuf> { anyhow::bail!("手机配置由原生存储管理") }
    pub fn atomic_write(_: &Path, _: &[u8]) -> anyhow::Result<()> { anyhow::bail!("手机配置由原生存储管理") }
}

pub mod log {
    // Mobile returns structured diagnostics, without logging subscription data.
    pub fn debug(_: &str, _: impl AsRef<str>) {}
    pub fn info(_: &str, _: impl AsRef<str>) {}
    pub fn warn(_: &str, _: impl AsRef<str>) {}
    pub fn error(_: &str, _: impl AsRef<str>) {}
}

pub mod backend { pub fn is_xray() -> bool { true } }
pub mod updates {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "lowercase")]
    pub enum UpdateChannel { Prod, Nightly, Xray }
}
pub mod controller {
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct LiveMember { pub name: String, pub delay: Option<u32> }
    #[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
    pub struct LiveGroup { pub name: String, pub kind: String, pub now: String, pub members: Vec<LiveMember> }
}
pub mod gfw {
    pub fn gfw_group(value: &str) -> Option<&str> { value.strip_prefix("gfw:") }
}
pub mod compile {
    pub const DNS_LISTEN_PORT: u16 = 1053;
    pub fn default_group(strategy: &crate::strategy::Strategy) -> &str { strategy.default_group_name() }
}

pub use catalog::{Catalog, Node};
pub use policy::{Decision, NodeHealth, Route};
pub use strategy::{Group, InboundMode, Matcher, RoutingProfile, Rule, RuleSet, Strategy, Subscription, GLOBAL_GROUP};
