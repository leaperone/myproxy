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
    let mut candidates: Vec<String> = names.into_iter().filter(|node| healthy(node, health)).collect();
    if item.kind == "select" && !item.selected.trim().is_empty() {
        let selected = item.selected.trim();
        if candidates.iter().any(|node| node == selected) {
            return Route::Node(selected.to_string());
        }
        if group(strategy, selected).is_some() {
            return choose_group(strategy, catalog, health, selected, visited, chain);
        }
    }
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
    if catalog.nodes.iter().any(|node| node.name == value) {
        return if healthy(value, health) { Route::Node(value.to_string()) } else { Route::Reject };
    }
    choose_group(strategy, catalog, health, value, &mut HashSet::new(), chain)
}

fn matcher_matches(kind: &str, value: &str, host: &str, process: Option<&str>) -> bool {
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let value = value.trim().trim_end_matches('.').to_ascii_lowercase();
    match kind.to_ascii_lowercase().as_str() {
        "domain" | "exact" => host == value,
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
    let width = if ip.is_ipv4() { 32 } else { 128 };
    if bits > width { return false; }
    let value = u128::from_be_bytes(match ip { IpAddr::V4(v) => v.to_ipv6_mapped().octets(), IpAddr::V6(v) => v.octets() });
    let base = u128::from_be_bytes(match network { IpAddr::V4(v) => v.to_ipv6_mapped().octets(), IpAddr::V6(v) => v.octets() });
    let mask = if bits == 0 { 0 } else { u128::MAX << (128 - u32::from(bits)) };
    (value & mask) == (base & mask)
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
    let mut rule = "unmatched".to_string();
    let route = match strategy.mixed_mode {
        InboundMode::Direct => Route::Direct,
        InboundMode::Global => target(strategy, catalog, health, &strategy.global_selected, &mut chain),
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
                RoutingProfile::Allowlist => target(strategy, catalog, health, &strategy.unmatched_via, &mut chain),
                RoutingProfile::Group => target(strategy, catalog, health, &strategy.unmatched_via, &mut chain),
                RoutingProfile::Gfwlist | RoutingProfile::Chinadirect => Route::Reject,
            })
        }
    };
    Decision { route, chain, rule }
}

pub fn groups(strategy: &Strategy, catalog: &Catalog, health: &HashMap<String, NodeHealth>) -> Vec<LiveGroup> {
    strategy.groups.iter().map(|group| {
        let names = members(group, catalog);
        let now = match choose_group(strategy, catalog, health, &group.name, &mut HashSet::new(), &mut Vec::new()) {
            Route::Node(name) => name,
            Route::Direct => "DIRECT".into(),
            Route::Reject => "REJECT".into(),
        };
        LiveGroup { name: group.name.clone(), kind: group.kind.clone(), now, members: names.into_iter().map(|name| LiveMember { delay: health.get(&name).and_then(|item| item.delay_ms), name }).collect() }
    }).collect()
}
