use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use crate::catalog::{self, Catalog};
use crate::controller::{LiveGroup, LiveMember};
use crate::strategy::{Group, InboundMode, RoutingProfile, Strategy};

#[derive(Clone, Debug, Default)]
pub struct NodeHealth {
    pub delay_ms: Option<u32>,
    pub failures: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Direct,
    Reject,
    Node(String),
}

#[derive(Clone, Debug)]
pub struct Decision {
    pub route: Route,
    pub chain: Vec<String>,
    pub rule: String,
}

fn group<'a>(strategy: &'a Strategy, name: &str) -> Option<&'a Group> {
    strategy.groups.iter().find(|item| item.name == name || item.id == name)
}

fn healthy(name: &str, health: &HashMap<String, NodeHealth>) -> bool {
    health.get(name).map_or(true, |item| item.failures < 2)
}

fn append_route(chain: &mut Vec<String>, route: &Route) {
    chain.push(match route {
        Route::Direct => "DIRECT".into(),
        Route::Reject => "REJECT".into(),
        Route::Node(name) => name.clone(),
    });
}

fn members(group: &Group, catalog: &Catalog) -> Vec<String> {
    catalog::resolve_group_members(group, catalog)
}

fn choose_group(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    name: &str,
    visited: &mut HashSet<String>,
    chain: &mut Vec<String>,
) -> Route {
    if !visited.insert(name.to_string()) { return Route::Reject; }
    let Some(item) = group(strategy, name) else { return Route::Reject };
    chain.push(item.name.clone());
    let names = members(item, catalog);
    let mut candidates: Vec<String> = names.into_iter().filter(|node| catalog.nodes.iter().any(|item| item.name == *node)).collect();
    if !item.selected.trim().is_empty() {
        let selected = item.selected.trim();
        if candidates.iter().any(|node| node == selected) {
            // A manual choice is authoritative, even while its probe is failing.
            return Route::Node(selected.to_string());
        }
        if group(strategy, selected).is_some() {
            return choose_group(strategy, catalog, health, selected, visited, chain);
        }
        return Route::Reject;
    }
    candidates.retain(|node| healthy(node, health));
    if item.kind == "url-test" {
        candidates.sort_by_key(|node| health.get(node).and_then(|value| value.delay_ms).unwrap_or(u32::MAX));
    }
    candidates.into_iter().next().map(Route::Node).unwrap_or(Route::Reject)
}

fn target(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    value: &str,
    chain: &mut Vec<String>,
) -> Route {
    let value = value.trim();
    if value.eq_ignore_ascii_case("direct") { return Route::Direct; }
    if value.eq_ignore_ascii_case("reject") { return Route::Reject; }
    if value.starts_with("gfw:") || value.starts_with("gfwlist:") { return Route::Reject; }
    let node_value = value.strip_prefix("node:").unwrap_or(value);
    if catalog.nodes.iter().any(|node| node.name == node_value) {
        return Route::Node(node_value.to_string());
    }
    if value.starts_with("node:") { return Route::Reject; }
    let group_value = value.strip_prefix("group:").unwrap_or(value);
    let group_value = if group_value.eq_ignore_ascii_case("default") || group_value.eq_ignore_ascii_case("proxy") { crate::compile::default_group(strategy) } else { group_value };
    choose_group(strategy, catalog, health, group_value, &mut HashSet::new(), chain)
}

fn matcher_matches(kind: &str, value: &str, host: &str, process: Option<&str>) -> bool {
    let wildcard_domain = value.trim().starts_with("*.");
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let value = value.trim().trim_end_matches('.').trim_start_matches("*.").trim_start_matches('.').to_ascii_lowercase();
    match kind.to_ascii_lowercase().as_str() {
        "domain" | "exact" => host == value || (wildcard_domain && host.ends_with(&format!(".{value}"))),
        "suffix" | "domain-suffix" => host == value || host.ends_with(&format!(".{value}")),
        "keyword" | "domain-keyword" => host.contains(&value),
        "app" | "process" => process.map(|item| item.eq_ignore_ascii_case(value.as_str())).unwrap_or(false),
        "cidr" | "ip" => host.parse::<IpAddr>().ok().map(|ip| ip_in_cidr(ip, &value)).unwrap_or(false),
        _ => false,
    }
}

fn ip_in_cidr(ip: IpAddr, cidr: &str) -> bool {
    let Some((raw_network, raw_bits)) = cidr.split_once('/') else { return false };
    let Ok(network) = raw_network.parse::<IpAddr>() else { return false };
    let Ok(bits) = raw_bits.parse::<u8>() else { return false };
    if ip.is_ipv4() != network.is_ipv4() { return false; }
    match (ip, network) {
        (IpAddr::V4(value), IpAddr::V4(base)) => {
            if bits > 32 { return false; }
            let mask = if bits == 0 { 0 } else { u32::MAX << (32 - u32::from(bits)) };
            (u32::from(value) & mask) == (u32::from(base) & mask)
        }
        (IpAddr::V6(value), IpAddr::V6(base)) => {
            if bits > 128 { return false; }
            let mask = if bits == 0 { 0 } else { u128::MAX << (128 - u32::from(bits)) };
            (u128::from_be_bytes(value.octets()) & mask) == (u128::from_be_bytes(base.octets()) & mask)
        }
        _ => false,
    }
}

pub fn decide(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    host: &str,
    _port: u16,
    process: Option<&str>,
) -> Decision {
    let mut chain = Vec::new();
    let mut rule = match strategy.mixed_mode {
        InboundMode::Global=>"全局出口",InboundMode::Proxy=>"默认组",InboundMode::Direct=>"全球直连",InboundMode::Rule=>"未匹配规则",
    }.to_string();
    let route = match strategy.mixed_mode {
        InboundMode::Direct => Route::Direct,
        InboundMode::Global => target(strategy, catalog, health, if strategy.global_selected.trim().is_empty() { crate::compile::default_group(strategy) } else { &strategy.global_selected }, &mut chain),
        InboundMode::Proxy => target(strategy, catalog, health, &crate::compile::default_group(strategy), &mut chain),
        InboundMode::Rule => {
            let mut selected = None;
            for set in &strategy.rule_sets {
                if set.matchers.iter().any(|matcher| matcher_matches(&matcher.kind, &matcher.value, host, process)) {
                    rule = set.name.clone();
                    selected = Some(target(strategy, catalog, health, &set.via, &mut chain));
                    break;
                }
            }
            selected.unwrap_or_else(|| match strategy.routing_profile {
                RoutingProfile::Allowlist => Route::Direct,
                RoutingProfile::Group => target(strategy, catalog, health, &strategy.unmatched_via, &mut chain),
                RoutingProfile::Gfwlist | RoutingProfile::Chinadirect => Route::Reject,
            })
        }
    };
    append_route(&mut chain, &route);
    Decision { route, chain, rule }
}

pub fn groups(strategy: &Strategy, catalog: &Catalog, health: &HashMap<String, NodeHealth>) -> Vec<LiveGroup> {
    let mut result = vec![LiveGroup { name: crate::strategy::GLOBAL_GROUP.into(), kind: "select".into(), now: if strategy.global_selected.trim().is_empty() { crate::compile::default_group(strategy).into() } else { strategy.global_selected.clone() }, members: strategy.groups.iter().map(|group| LiveMember { name: group.name.clone(), delay: None }).chain(catalog.nodes.iter().map(|node| LiveMember { name: node.name.clone(), delay: health.get(&node.name).and_then(|item| item.delay_ms) })).chain([LiveMember { name: "DIRECT".into(), delay: None }, LiveMember { name: "REJECT".into(), delay: None }]).collect() }];
    result.extend(strategy.groups.iter().map(|group| {
        let names = members(group, catalog);
        let now = match choose_group(strategy, catalog, health, &group.name, &mut HashSet::new(), &mut Vec::new()) {
            Route::Node(name) => name,
            Route::Direct => "DIRECT".into(),
            Route::Reject => "REJECT".into(),
        };
        LiveGroup { name: group.name.clone(), kind: group.kind.clone(), now, members: names.into_iter().map(|name| LiveMember { delay: health.get(&name).and_then(|item| item.delay_ms), name }).collect() }
    }));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;
    use crate::catalog::Node;
    use crate::strategy::{Matcher, RuleSet};

    fn fixture() -> (Strategy, Catalog) {
        let mut strategy = Strategy::default();
        strategy.groups = vec![Group::all_nodes("AUTO".into(), "url-test".into()), Group::all_nodes("MANUAL".into(), "select".into())];
        strategy.mixed_mode = InboundMode::Global;
        strategy.global_selected = "AUTO".into();
        let catalog = Catalog { nodes: vec![Node { name: "A".into(), subscription: "s".into(), raw: Value::Null }, Node { name: "B".into(), subscription: "s".into(), raw: Value::Null }], ..Catalog::default() };
        (strategy, catalog)
    }

    #[test]
    fn url_test_and_failed_nodes() {
        let (strategy, catalog) = fixture();
        let health = HashMap::from([(String::from("A"), NodeHealth { delay_ms: Some(100), failures: 0 }), (String::from("B"), NodeHealth { delay_ms: Some(10), failures: 0 })]);
        assert_eq!(decide(&strategy, &catalog, &health, "example.com", 443, None).route, Route::Node("B".into()));
        let failed = HashMap::from([(String::from("A"), NodeHealth { failures: 2, ..Default::default() }), (String::from("B"), NodeHealth { failures: 2, ..Default::default() })]);
        assert_eq!(decide(&strategy, &catalog, &failed, "example.com", 443, None).route, Route::Reject);
    }

    #[test]
    fn manual_override_is_authoritative_for_every_group_kind() {
        let (mut strategy, catalog) = fixture();
        strategy.groups[0].selected = "A".into();
        let health = HashMap::from([(String::from("A"), NodeHealth { failures: 99, ..Default::default() })]);
        assert_eq!(decide(&strategy, &catalog, &health, "example.com", 443, None).route, Route::Node("A".into()));
    }

    #[test]
    fn matchers_are_or_and_cidr_is_real_ipv4() {
        let (mut strategy, catalog) = fixture();
        strategy.mixed_mode = InboundMode::Rule;
        strategy.routing_profile = RoutingProfile::Allowlist;
        strategy.rule_sets = vec![RuleSet { id: "r".into(), name: "test".into(), via: "AUTO".into(), matchers: vec![Matcher { kind: "suffix".into(), value: "*.example.com".into() }, Matcher { kind: "cidr".into(), value: "10.0.0.0/8".into() }] }];
        let health = HashMap::new();
        assert_eq!(decide(&strategy, &catalog, &health, "www.example.com", 443, None).rule, "test");
        assert!(matcher_matches("cidr", "10.0.0.0/8", "10.2.3.4", None));
        assert!(!matcher_matches("cidr", "10.0.0.0/8", "11.2.3.4", None));
        assert_eq!(decide(&strategy, &catalog, &health, "other.invalid", 443, None).route, Route::Direct);
    }

    #[test]
    fn global_and_cycle_fail_closed_and_live_groups_include_reserved_targets() {
        let (mut strategy, catalog) = fixture();
        strategy.global_selected = "missing".into();
        assert_eq!(decide(&strategy, &catalog, &HashMap::new(), "x", 443, None).route, Route::Reject);
        let live = groups(&strategy, &catalog, &HashMap::new());
        assert_eq!(live[0].name, "GLOBAL");
        assert!(live[0].members.iter().any(|item| item.name == "DIRECT"));
        assert!(live[0].members.iter().any(|item| item.name == "AUTO"));
    }
    #[test]
    fn fallback_keeps_priority_and_manual_override_until_auto_is_restored() {
        let (mut strategy,catalog)=fixture();
        strategy.groups[0].kind="fallback".into();
        let mut health=HashMap::from([("A".into(),NodeHealth { delay_ms:Some(90),failures:0 }), ("B".into(),NodeHealth { delay_ms:Some(10),failures:0 })]);
        assert_eq!(decide(&strategy,&catalog,&health,"public.invalid",443,None).route,Route::Node("A".into()));
        health.get_mut("A").unwrap().failures=2;
        assert_eq!(decide(&strategy,&catalog,&health,"public.invalid",443,None).route,Route::Node("B".into()));
        for kind in ["select","fallback","url-test"] {
            strategy.groups[0].kind=kind.into();strategy.groups[0].selected="A".into();
            assert_eq!(decide(&strategy,&catalog,&health,"public.invalid",443,None).route,Route::Node("A".into()));
        }
        strategy.groups[0].selected.clear();
        assert_eq!(decide(&strategy,&catalog,&health,"public.invalid",443,None).route,Route::Node("B".into()));
    }

}
