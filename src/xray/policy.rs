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
    pub allow_direct_fallback: bool,
    pub route: Route,
    pub chain: Vec<String>,
    pub rule: String,
}

pub struct FlowContext<'a> {
    pub host: &'a str,
    pub hostname: Option<&'a str>,
    pub port: u16,
    pub network: &'a str,
    pub user_id: Option<u32>,
    pub applications: &'a [String],
}

pub fn application_identifiers(path: Option<&str>, bundle: Option<&str>, signing: Option<&str>) -> Vec<String> {
    let mut values = Vec::new();
    for value in [path, bundle, signing].into_iter().flatten() {
        let value = value.trim().to_lowercase();
        if !value.is_empty() && !values.contains(&value) { values.push(value); }
    }
    if let Some(path) = path {
        for component in path.split('/').filter(|part| !part.is_empty()) {
            if component.to_ascii_lowercase().ends_with(".app") {
                values.push(component.to_lowercase());
                values.push(component[..component.len()-4].to_lowercase());
            }
        }
        if let Some(name) = std::path::Path::new(path).file_name().and_then(|name| name.to_str()) {
            values.push(name.to_lowercase());
        }
    }
    values.sort();
    values.dedup();
    values
}

fn application_matches(pattern: &str, applications: &[String]) -> bool {
    let patterns = application_identifiers(Some(pattern.trim()), None, None);
    patterns.iter().any(|pattern| applications.iter().any(|value|
        catalog::wildcard_match(&pattern.chars().collect::<Vec<_>>(), &value.to_lowercase().chars().collect::<Vec<_>>())))
}

fn group<'a>(strategy: &'a Strategy, name: &str) -> Option<&'a Group> {
    strategy
        .groups
        .iter()
        .find(|item| item.name.eq_ignore_ascii_case(name.trim()) || item.id == name.trim())
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

fn members(strategy: &Strategy, item: &Group, catalog: &Catalog) -> Vec<String> {
    catalog::resolve_group_members(item, catalog).into_iter().map(|name| {
        group(strategy, &name).map_or(name.clone(), |child| child.name.clone())
    }).collect()
}

fn choose_group(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    name: &str,
    visited: &mut HashSet<String>,
    chain: &mut Vec<String>,
    cache: &mut HashMap<String, (Route, Vec<String>)>,
) -> Route {
    let Some(item) = group(strategy, name) else { return Route::Reject; };
    if visited.len() >= 64 || visited.contains(&item.id) { return Route::Reject; }
    if let Some((route, path)) = cache.get(&item.id) {
        if visited.len() + path.len() > 64 { return Route::Reject; }
        chain.extend(path.iter().cloned());
        return route.clone();
    }
    visited.insert(item.id.clone());
    let chain_start = chain.len();
    chain.push(item.name.clone());
    let choices = members(strategy, item, catalog);
    let selected = item.selected.trim();
    let route = if !selected.is_empty() {
        if choices.iter().any(|value| value == selected)
            && catalog.nodes.iter().any(|node| node.name == selected) {
            Route::Node(selected.to_string())
        } else if group(strategy, selected).is_some()
            && (item.group_refs.is_empty() || item.group_refs.iter().any(|name| name.trim().eq_ignore_ascii_case(selected))) {
            choose_group(strategy, catalog, health, selected, visited, chain, cache)
        } else {
            Route::Reject
        }
    } else {
        let mut best: Option<(Route, Vec<String>, u32)> = None;
        for choice in choices {
            let mut child_chain = Vec::new();
            let candidate = if group(strategy, &choice).is_some() {
                choose_group(strategy, catalog, health, &choice, visited, &mut child_chain, cache)
            } else if catalog.nodes.iter().any(|node| node.name == choice) {
                Route::Node(choice)
            } else {
                Route::Reject
            };
            let Route::Node(node) = &candidate else { continue; };
            if !healthy(node, health) { continue; }
            let delay = health.get(node).and_then(|value| value.delay_ms).unwrap_or(u32::MAX);
            if best.as_ref().is_none_or(|(_, _, previous)| delay < *previous) {
                best = Some((candidate, child_chain, delay));
            }
            if item.kind != "url-test" { break; }
        }
        if let Some((route, child_chain, _)) = best {
            chain.extend(child_chain);
            route
        } else {
            Route::Reject
        }
    };
    visited.remove(&item.id);
    cache.insert(item.id.clone(), (route.clone(), chain[chain_start..].to_vec()));
    route
}

fn target(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    value: &str,
    chain: &mut Vec<String>,
) -> Route {
    let value = value.trim();
    if value.eq_ignore_ascii_case("direct") {
        return Route::Direct;
    }
    if value.eq_ignore_ascii_case("reject") {
        return Route::Reject;
    }
    if value.starts_with("gfw:") || value.starts_with("gfwlist:") {
        return Route::Reject;
    }
    let node_value = value.strip_prefix("node:").unwrap_or(value);
    if catalog.nodes.iter().any(|node| node.name == node_value) {
        return Route::Node(node_value.to_string());
    }
    if value.starts_with("node:") {
        return Route::Reject;
    }
    let group_value = value.strip_prefix("group:").unwrap_or(value);
    let group_value = if group_value.eq_ignore_ascii_case("default")
        || group_value.eq_ignore_ascii_case("proxy")
    {
        crate::compile::default_group(strategy)
    } else {
        group_value
    };
    choose_group(
        strategy,
        catalog,
        health,
        group_value,
        &mut HashSet::new(),
        chain,
        &mut HashMap::new(),
    )
}

fn matcher_matches(kind: &str, value: &str, host: &str, process: Option<&str>) -> bool {
    let wildcard_domain = value.trim().starts_with("*.");
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    if kind == "wildcard" {
        let pattern = value.trim().trim_end_matches('.').to_ascii_lowercase();
        if let Some(suffix) = pattern.strip_prefix('*') {
            if !suffix.contains(['*', '?']) {
                return host.ends_with(suffix);
            }
        }
        return catalog::wildcard_match(
            &pattern.chars().collect::<Vec<_>>(),
            &host.chars().collect::<Vec<_>>(),
        );
    }
    let value = value
        .trim()
        .trim_end_matches('.')
        .trim_start_matches("*.")
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match kind.to_ascii_lowercase().as_str() {
        "domain" | "exact" => {
            host == value || (wildcard_domain && host.ends_with(&format!(".{value}")))
        }
        "suffix" | "domain-suffix" => host == value || host.ends_with(&format!(".{value}")),
        "keyword" | "domain-keyword" => host.contains(&value),
        "app" | "process" => process
            .map(|item| item.eq_ignore_ascii_case(value.as_str()))
            .unwrap_or(false),
        "cidr" | "ip" => host
            .parse::<IpAddr>()
            .ok()
            .map(|ip| ip_in_cidr(ip, &value))
            .unwrap_or(false),
        _ => false,
    }
}

pub fn rule_matches(
    set: &crate::strategy::RuleSet,
    host: &str,
    process: Option<&str>,
    network: &str,
    port: u16,
) -> anyhow::Result<bool> {
    let applications = process.map(|value| vec![value.to_lowercase()]).unwrap_or_default();
    rule_matches_context(set, &FlowContext { host, hostname: None, port, network, user_id: None, applications: &applications })
}

fn rule_matches_context(set: &crate::strategy::RuleSet, context: &FlowContext<'_>) -> anyhow::Result<bool> {
    let has_network = set.matchers.iter().any(|matcher| matcher.kind == "network");
    if has_network && !set.matchers.iter().any(|matcher| {
            matcher.kind == "network" && matcher.value.eq_ignore_ascii_case(context.network)
        }) {
        return Ok(false);
    }
    let users = set.matchers.iter().filter(|matcher| matcher.kind == "uid").collect::<Vec<_>>();
    if !users.is_empty() && !users.iter().any(|matcher| context.user_id.is_some_and(|uid| matcher.value.parse::<u32>().ok() == Some(uid))) { return Ok(false); }
    let ports = set.matchers.iter().filter(|matcher| matcher.kind == "port").collect::<Vec<_>>();
    if !ports.is_empty() && !ports.iter().any(|matcher| matcher.value.parse::<u16>().ok() == Some(context.port)) { return Ok(false); }
    let mut has_target = false;
    let literal_ip = context.host.parse::<IpAddr>().ok();
    let hostname = context.hostname.filter(|name| name.parse::<IpAddr>().is_err())
        .or_else(|| literal_ip.is_none().then_some(context.host));
    for matcher in &set.matchers {
        if matches!(matcher.kind.as_str(), "network" | "port" | "uid") { continue; }
        has_target = true;
        if matches!(matcher.kind.as_str(), "app" | "process") {
            if application_matches(&matcher.value, context.applications) { return Ok(true); }
            continue;
        }
        let host = if matches!(matcher.kind.as_str(), "cidr" | "ip" | "geo-ip") {
            if literal_ip.is_none() { continue; }
            context.host
        } else {
            let Some(hostname) = hostname else { continue; };
            hostname
        };
        let matched = if matches!(matcher.kind.as_str(), "geo-site" | "geo-ip") {
            super::geo::matches(&matcher.kind, &matcher.value, host)?
        } else {
            matcher_matches(&matcher.kind, &matcher.value, host, None)
        };
        if matched { return Ok(true); }
    }
    Ok(!has_target)
}

fn ip_in_cidr(ip: IpAddr, cidr: &str) -> bool {
    let Some((raw_network, raw_bits)) = cidr.split_once('/') else {
        return false;
    };
    let Ok(network) = raw_network.parse::<IpAddr>() else {
        return false;
    };
    let Ok(bits) = raw_bits.parse::<u8>() else {
        return false;
    };
    if ip.is_ipv4() != network.is_ipv4() {
        return false;
    }
    match (ip, network) {
        (IpAddr::V4(value), IpAddr::V4(base)) => {
            if bits > 32 {
                return false;
            }
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(bits))
            };
            (u32::from(value) & mask) == (u32::from(base) & mask)
        }
        (IpAddr::V6(value), IpAddr::V6(base)) => {
            if bits > 128 {
                return false;
            }
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(bits))
            };
            (u128::from_be_bytes(value.octets()) & mask)
                == (u128::from_be_bytes(base.octets()) & mask)
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
    decide_network(strategy, catalog, health, host, _port, process, "tcp")
}

pub fn decide_network(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    host: &str,
    _port: u16,
    process: Option<&str>,
    network: &str,
) -> Decision {
    decide_with_mode(strategy, catalog, health, host, _port, process, network, strategy.mixed_mode)
}

pub fn decide_capture(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    host: &str,
    port: u16,
    network: &str,
) -> Decision {
    decide_with_mode(strategy, catalog, health, host, port, None, network, strategy.extension_mode)
}

pub fn decide_target(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    value: &str,
) -> Decision {
    let mut chain = Vec::new();
    let route = target(strategy, catalog, health, value, &mut chain);
    append_route(&mut chain, &route);
    Decision { allow_direct_fallback: false, route, chain, rule: "应用规则".into() }
}

fn decide_rule(strategy: &Strategy, catalog: &Catalog, health: &HashMap<String, NodeHealth>, set: &crate::strategy::RuleSet) -> Decision {
    let mut decision = decide_target(strategy, catalog, health, &set.via);
    decision.rule = set.name.clone();
    decision.allow_direct_fallback = set.unavailable_fallback == Some(crate::strategy::UnavailableFallback::Direct)
        && !set.via.eq_ignore_ascii_case("reject");
    if decision.route == Route::Reject && decision.allow_direct_fallback {
        decision.route = Route::Direct;
        decision.chain.pop();
        decision.chain.push("DIRECT".into());
    }
    decision
}

pub fn decide_application(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    context: &FlowContext<'_>,
) -> Decision {
    let native = match context.host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || ip.is_link_local() || ip.is_broadcast()
            || (!strategy.lan_capture && ip.is_private()),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || ip.is_unicast_link_local()
            || (!strategy.lan_capture && ip.is_unique_local()),
        Err(_) => false,
    };
    let local_name = [Some(context.host), context.hostname].into_iter().flatten().any(|name| {
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        name == "localhost" || [".local", ".lan", ".localdomain", ".home.arpa"].iter().any(|suffix| name.ends_with(suffix))
    });
    if native || local_name {
        return Decision { allow_direct_fallback: false, route: Route::Direct, chain: vec!["DIRECT".into()], rule: "本机与局域网".into() };
    }
    decide_context(strategy, catalog, health, context, strategy.extension_mode)
}

fn decide_with_mode(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    host: &str,
    _port: u16,
    process: Option<&str>,
    network: &str,
    mode: InboundMode,
) -> Decision {
    let applications = process.map(|value| vec![value.to_lowercase()]).unwrap_or_default();
    let context = FlowContext { host, hostname: None, port: _port, network, user_id: None, applications: &applications };
    decide_context(strategy, catalog, health, &context, mode)
}

fn decide_context(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
    context: &FlowContext<'_>,
    mode: InboundMode,
) -> Decision {
    let mut chain = Vec::new();
    let rule = match mode {
        InboundMode::Global => "全局出口",
        InboundMode::Proxy => "默认组",
        InboundMode::Direct => "全球直连",
        InboundMode::Rule => "未匹配规则",
    }
    .to_string();
    let route =
        match mode {
            InboundMode::Direct => Route::Direct,
            InboundMode::Global => target(
                strategy,
                catalog,
                health,
                if strategy.global_selected.trim().is_empty() {
                    crate::compile::default_group(strategy)
                } else {
                    &strategy.global_selected
                },
                &mut chain,
            ),
            InboundMode::Proxy => target(
                strategy,
                catalog,
                health,
                &crate::compile::default_group(strategy),
                &mut chain,
            ),
            InboundMode::Rule => {
                for set in &strategy.rule_sets {
                    let matched = match rule_matches_context(set, context) {
                        Ok(matched) => matched,
                        Err(_) => return Decision { allow_direct_fallback: false, route: Route::Reject, chain: vec!["REJECT".into()], rule: "规则数据库不可用".into() },
                    };
                    if matched {
                        return decide_rule(strategy, catalog, health, set);
                    }
                }
                match strategy.routing_profile {
                    RoutingProfile::Allowlist => Route::Direct,
                    RoutingProfile::Group => target(
                        strategy,
                        catalog,
                        health,
                        &strategy.unmatched_via,
                        &mut chain,
                    ),
                    RoutingProfile::Gfwlist | RoutingProfile::Chinadirect => Route::Reject,
                }
            }
        };
    append_route(&mut chain, &route);
    Decision { allow_direct_fallback: false, route, chain, rule }
}

pub fn groups(
    strategy: &Strategy,
    catalog: &Catalog,
    health: &HashMap<String, NodeHealth>,
) -> Vec<LiveGroup> {
    let mut result = vec![LiveGroup {
        name: crate::strategy::GLOBAL_GROUP.into(),
        kind: "select".into(),
        now: if strategy.global_selected.trim().is_empty() {
            crate::compile::default_group(strategy).into()
        } else {
            strategy.global_selected.clone()
        },
        members: strategy
            .groups
            .iter()
            .map(|group| LiveMember {
                name: group.name.clone(),
                delay: None,
            })
            .chain(catalog.nodes.iter().map(|node| LiveMember {
                name: node.name.clone(),
                delay: health.get(&node.name).and_then(|item| item.delay_ms),
            }))
            .chain([
                LiveMember {
                    name: "DIRECT".into(),
                    delay: None,
                },
                LiveMember {
                    name: "REJECT".into(),
                    delay: None,
                },
            ])
            .collect(),
    }];
    let mut cache = HashMap::new();
    result.extend(strategy.groups.iter().map(|group| {
        let names = members(strategy, group, catalog);
        let mut chain = Vec::new();
        let now = match choose_group(
            strategy,
            catalog,
            health,
            &group.name,
            &mut HashSet::new(),
            &mut chain,
            &mut cache,
        ) {
            Route::Node(name) => chain.get(1).cloned().unwrap_or(name),
            Route::Direct => "DIRECT".into(),
            Route::Reject => "REJECT".into(),
        };
        LiveGroup {
            name: group.name.clone(),
            kind: group.kind.clone(),
            now,
            members: names
                .into_iter()
                .map(|name| LiveMember {
                    delay: if strategy.groups.iter().any(|item| item.name == name) {
                        let route = choose_group(strategy, catalog, health, &name, &mut HashSet::new(), &mut Vec::new(), &mut cache);
                        if let Route::Node(node) = route { health.get(&node).and_then(|item| item.delay_ms) } else { None }
                    } else { health.get(&name).and_then(|item| item.delay_ms) },
                    name,
                })
                .collect(),
        }
    }));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xray_application_decisions_preserve_source_target_order_and_qualifiers() {
        use crate::strategy::{Matcher, RuleSet};
        let mut strategy = crate::xray::default_strategy();
        strategy.extension_mode = InboundMode::Rule;
        strategy.routing_profile = RoutingProfile::Allowlist;
        strategy.rule_sets = vec![RuleSet {
            id:"combined".into(), name:"Application or website".into(), via:"REJECT".into(), unavailable_fallback:None,
            matchers:vec![Matcher::app("*cursor*".into()), Matcher {kind:"suffix".into(),value:"example.com".into()}, Matcher {kind:"network".into(),value:"tcp".into()}, Matcher {kind:"port".into(),value:"443".into()}],
        }];
        let apps = application_identifiers(Some("/Applications/Cursor.app/Contents/Frameworks/Cursor Helper.app/Contents/MacOS/Cursor Helper"), Some("com.todesktop.cursor"), Some("com.cursor.helper"));
        let catalog = Catalog::default(); let health = HashMap::new();
        for (source, hostname, port, network, expected) in [
            (true,"other.test",443,"tcp",Route::Reject),
            (false,"www.example.com",443,"tcp",Route::Reject),
            (false,"other.test",443,"tcp",Route::Direct),
            (true,"www.example.com",80,"tcp",Route::Direct),
            (true,"www.example.com",443,"udp",Route::Direct),
        ] {
            let context = FlowContext {host:"203.0.113.8",hostname:Some(hostname),port,network,user_id:Some(501),applications:if source {&apps} else {&[]}};
            assert_eq!(decide_application(&strategy,&catalog,&health,&context).route,expected);
        }
        strategy.rule_sets.insert(0, RuleSet {id:"first".into(),name:"Website direct".into(),via:"DIRECT".into(),unavailable_fallback:None,matchers:vec![Matcher {kind:"suffix".into(),value:"example.com".into()}]});
        let context = FlowContext {host:"203.0.113.8",hostname:Some("www.example.com"),port:443,network:"tcp",user_id:Some(501),applications:&apps};
        assert_eq!(decide_application(&strategy,&catalog,&health,&context).route,Route::Direct);
    }

    #[test]
    fn xray_uid_rules_use_verified_uid_and_keep_both_ip_and_hostname() {
        use crate::strategy::{Matcher, RuleSet};
        let set = RuleSet {id:"qualified".into(),name:"Qualified".into(),via:"REJECT".into(),unavailable_fallback:None,
            matchers:vec![Matcher {kind:"uid".into(),value:"501".into()},Matcher::cidr("203.0.113.0/24".into()),Matcher {kind:"port".into(),value:"443".into()}]};
        let mut context = FlowContext {host:"203.0.113.8",hostname:Some("example.com"),port:443,network:"tcp",user_id:Some(501),applications:&[]};
        assert!(rule_matches_context(&set,&context).unwrap());
        context.user_id = None;
        assert!(!rule_matches_context(&set,&context).unwrap());
        context.user_id = Some(502);
        assert!(!rule_matches_context(&set,&context).unwrap());
        context.user_id = Some(501); context.host="198.51.100.8";
        assert!(!rule_matches_context(&set,&context).unwrap());
        let kernel_identifiers = application_identifiers(None,None,Some("com.example.app"));
        assert!(application_matches("com.example.app",&kernel_identifiers));
        assert!(!application_matches("/Applications/Other.app/Contents/MacOS/Other",&kernel_identifiers));
    }

    #[test]
    fn imported_user_rules_cannot_match_clients_without_source_identity() {
        let set=crate::strategy::RuleSet {unavailable_fallback:None,id:"dns".into(),name:"DNS guard".into(),via:"DIRECT".into(),matchers:vec![crate::strategy::Matcher {kind:"uid".into(),value:"0".into()},crate::strategy::Matcher::cidr("192.0.2.0/24".into()),crate::strategy::Matcher {kind:"port".into(),value:"53".into()}]};
        assert!(!rule_matches(&set,"192.0.2.1",None,"udp",53).unwrap());
        let mut context = FlowContext { host:"192.0.2.1", hostname:None, port:53, network:"udp", user_id:Some(0), applications:&[] };
        assert!(rule_matches_context(&set, &context).unwrap());
        context.host = "198.51.100.1";
        assert!(!rule_matches_context(&set, &context).unwrap());
        context.host = "192.0.2.1";
        context.port = 443;
        assert!(!rule_matches_context(&set, &context).unwrap());
        context.port = 53;
        context.user_id = None;
        assert!(!rule_matches_context(&set, &context).unwrap());
    }

    #[test]
    fn imported_domain_masks_preserve_literal_suffix_and_apex_boundaries() {
        for (pattern, host, expected) in [
            ("*.example.com", "example.com", false),
            ("*.example.com", "www.example.com", true),
            ("*example.com", "example.com", true),
            ("*example.com", "prefixexample.com", true),
            ("*example.com", "example.com.evil.invalid", false),
            ("*example*", "www.example.invalid", true),
            ("api?.example.com", "api1.example.com", true),
            ("api?.example.com", "api12.example.com", false),
        ] {
            assert_eq!(matcher_matches("wildcard", pattern, host, None), expected, "{pattern} / {host}");
        }
    }

    #[test]
    fn transport_qualifies_a_rule_instead_of_matching_every_tcp_destination() {
        let set = crate::strategy::RuleSet {
            unavailable_fallback: Default::default(),
            id: "qualified".into(),
            name: "Qualified destination".into(),
            via: "DIRECT".into(),
            matchers: vec![
                crate::strategy::Matcher { kind: "wildcard".into(), value: "*example.com".into() },
                crate::strategy::Matcher { kind: "network".into(), value: "tcp".into() },
            ],
        };
        assert!(rule_matches(&set, "www.example.com", None, "tcp", 443).unwrap());
        assert!(!rule_matches(&set, "www.example.com", None, "udp", 443).unwrap());
        assert!(!rule_matches(&set, "other.invalid", None, "tcp", 443).unwrap());
    }
    use crate::catalog::Node;
    use crate::strategy::{Matcher, RuleSet};
    use serde_yaml::Value;

    fn fixture() -> (Strategy, Catalog) {
        let mut strategy = Strategy::default();
        strategy.groups = vec![
            Group::all_nodes("AUTO".into(), "url-test".into()),
            Group::all_nodes("MANUAL".into(), "select".into()),
        ];
        strategy.mixed_mode = InboundMode::Global;
        strategy.global_selected = "AUTO".into();
        let catalog = Catalog {
            nodes: vec![
                Node {
                    name: "A".into(),
                    subscription: "s".into(),
                    raw: Value::Null,
                },
                Node {
                    name: "B".into(),
                    subscription: "s".into(),
                    raw: Value::Null,
                },
            ],
            ..Catalog::default()
        };
        (strategy, catalog)
    }

    #[test]
    fn url_test_and_failed_nodes() {
        let (strategy, catalog) = fixture();
        let health = HashMap::from([
            (
                String::from("A"),
                NodeHealth {
                    delay_ms: Some(100),
                    failures: 0,
                },
            ),
            (
                String::from("B"),
                NodeHealth {
                    delay_ms: Some(10),
                    failures: 0,
                },
            ),
        ]);
        assert_eq!(
            decide(&strategy, &catalog, &health, "example.com", 443, None).route,
            Route::Node("B".into())
        );
        let failed = HashMap::from([
            (
                String::from("A"),
                NodeHealth {
                    failures: 2,
                    ..Default::default()
                },
            ),
            (
                String::from("B"),
                NodeHealth {
                    failures: 2,
                    ..Default::default()
                },
            ),
        ]);
        assert_eq!(
            decide(&strategy, &catalog, &failed, "example.com", 443, None).route,
            Route::Reject
        );
    }

    #[test]
    fn manual_override_is_authoritative_for_every_group_kind() {
        let (mut strategy, catalog) = fixture();
        strategy.groups[0].selected = "A".into();
        let health = HashMap::from([(
            String::from("A"),
            NodeHealth {
                failures: 99,
                ..Default::default()
            },
        )]);
        assert_eq!(
            decide(&strategy, &catalog, &health, "example.com", 443, None).route,
            Route::Node("A".into())
        );
    }

    #[test]
    fn matchers_are_or_and_cidr_is_real_ipv4() {
        let (mut strategy, catalog) = fixture();
        strategy.mixed_mode = InboundMode::Rule;
        strategy.routing_profile = RoutingProfile::Allowlist;
        strategy.rule_sets = vec![RuleSet {
            unavailable_fallback: Default::default(),
            id: "r".into(),
            name: "test".into(),
            via: "AUTO".into(),
            matchers: vec![
                Matcher {
                    kind: "suffix".into(),
                    value: "*.example.com".into(),
                },
                Matcher {
                    kind: "cidr".into(),
                    value: "10.0.0.0/8".into(),
                },
            ],
        }];
        let health = HashMap::new();
        assert_eq!(
            decide(&strategy, &catalog, &health, "www.example.com", 443, None).rule,
            "test"
        );
        assert!(matcher_matches("cidr", "10.0.0.0/8", "10.2.3.4", None));
        assert!(!matcher_matches("cidr", "10.0.0.0/8", "11.2.3.4", None));
        assert_eq!(
            decide(&strategy, &catalog, &health, "other.invalid", 443, None).route,
            Route::Direct
        );
    }

    #[test]
    fn global_and_cycle_fail_closed_and_live_groups_include_reserved_targets() {
        let (mut strategy, catalog) = fixture();
        strategy.global_selected = "missing".into();
        assert_eq!(
            decide(&strategy, &catalog, &HashMap::new(), "x", 443, None).route,
            Route::Reject
        );
        let live = groups(&strategy, &catalog, &HashMap::new());
        assert_eq!(live[0].name, "GLOBAL");
        assert!(live[0].members.iter().any(|item| item.name == "DIRECT"));
        assert!(live[0].members.iter().any(|item| item.name == "AUTO"));
    }
    #[test]
    fn fallback_keeps_priority_and_manual_override_until_auto_is_restored() {
        let (mut strategy, catalog) = fixture();
        strategy.groups[0].kind = "fallback".into();
        let mut health = HashMap::from([
            (
                "A".into(),
                NodeHealth {
                    delay_ms: Some(90),
                    failures: 0,
                },
            ),
            (
                "B".into(),
                NodeHealth {
                    delay_ms: Some(10),
                    failures: 0,
                },
            ),
        ]);
        assert_eq!(
            decide(&strategy, &catalog, &health, "public.invalid", 443, None).route,
            Route::Node("A".into())
        );
        health.get_mut("A").unwrap().failures = 2;
        assert_eq!(
            decide(&strategy, &catalog, &health, "public.invalid", 443, None).route,
            Route::Node("B".into())
        );
        for kind in ["select", "fallback", "url-test"] {
            strategy.groups[0].kind = kind.into();
            strategy.groups[0].selected = "A".into();
            assert_eq!(
                decide(&strategy, &catalog, &health, "public.invalid", 443, None).route,
                Route::Node("A".into())
            );
        }
        strategy.groups[0].selected.clear();
        assert_eq!(
            decide(&strategy, &catalog, &health, "public.invalid", 443, None).route,
            Route::Node("B".into())
        );
    }
    fn regional_fixture() -> (Strategy, Catalog, HashMap<String, NodeHealth>) {
        let strategy = crate::xray::default_strategy();
        let values = [("美国 slow", 90), ("美国 fast", 40), ("日本 fast", 10), ("香港 fast", 1)];
        let catalog = Catalog {
            nodes: values.iter().map(|(name, _)| crate::catalog::Node {
                name: (*name).into(), subscription: "fixture".into(), raw: serde_yaml::Value::Null,
            }).collect(),
            ..Catalog::default()
        };
        let health = values.into_iter().map(|(name, delay)| (name.into(), NodeHealth {
            delay_ms: Some(delay), failures: 0,
        })).collect();
        (strategy, catalog, health)
    }

    #[test]
    fn region_priority_beats_other_region_latency_and_fails_over_then_recovers() {
        let (strategy, catalog, mut health) = regional_fixture();
        let pick = |health: &HashMap<String, NodeHealth>| decide(&strategy, &catalog, health, "example.com", 443, None);
        let decision = pick(&health);
        assert_eq!(decision.route, Route::Node("美国 fast".into()));
        assert_eq!(decision.chain, ["节点选择", "美国优先", "美国", "美国 fast"]);
        health.get_mut("美国 slow").unwrap().failures = 2;
        health.get_mut("美国 fast").unwrap().failures = 2;
        let decision = pick(&health);
        assert_eq!(decision.route, Route::Node("日本 fast".into()));
        assert_eq!(decision.chain, ["节点选择", "美国优先", "日本", "日本 fast"]);
        health.get_mut("日本 fast").unwrap().failures = 2;
        assert_eq!(pick(&health).route, Route::Node("香港 fast".into()));
        health.get_mut("香港 fast").unwrap().failures = 2;
        assert_eq!(pick(&health).route, Route::Reject);
        health.get_mut("美国 slow").unwrap().failures = 0;
        assert_eq!(pick(&health).route, Route::Node("美国 slow".into()));
    }

    #[test]
    fn regional_profiles_keep_their_own_order_and_live_child_selection() {
        let (mut strategy, catalog, health) = regional_fixture();
        strategy.global_selected = "日本优先".into();
        assert_eq!(decide(&strategy, &catalog, &health, "example.com", 443, None).route, Route::Node("日本 fast".into()));
        strategy.global_selected = "香港优先".into();
        assert_eq!(decide(&strategy, &catalog, &health, "example.com", 443, None).route, Route::Node("香港 fast".into()));
        let live = groups(&strategy, &catalog, &health);
        let profile = live.iter().find(|group| group.name == "美国优先").unwrap();
        assert_eq!(profile.now, "美国");
        assert_eq!(profile.members.iter().map(|member| member.name.as_str()).collect::<Vec<_>>(), ["美国", "日本", "香港"]);
        assert_eq!(live.iter().find(|group| group.name == "美国").unwrap().now, "美国 fast");
        assert_eq!(live.iter().find(|group| group.name == "节点选择").unwrap().now, "美国优先");
    }

    #[test]
    fn nested_manual_selection_and_shared_latency_groups_use_actual_routes() {
        let (mut strategy, catalog, mut health) = regional_fixture();
        strategy.groups[0].selected = "日本优先".into();
        assert_eq!(decide(&strategy, &catalog, &health, "example.com", 443, None).route, Route::Node("日本 fast".into()));
        strategy.groups[0].selected = "美国 slow".into();
        health.get_mut("美国 slow").unwrap().failures = 99;
        assert_eq!(decide(&strategy, &catalog, &health, "example.com", 443, None).route, Route::Node("美国 slow".into()));
        strategy.groups[0].selected.clear();
        strategy.groups[0].all_nodes = false;
        strategy.groups[0].kind = "url-test".into();
        let decision = decide(&strategy, &catalog, &health, "example.com", 443, None);
        assert_eq!(decision.route, Route::Node("香港 fast".into()));
        assert_eq!(decision.chain, ["节点选择", "香港优先", "香港", "香港 fast"]);
    }

    #[test]
    fn default_selector_can_use_a_node_outside_the_regional_presets() {
        let strategy = crate::xray::default_strategy();
        let catalog = Catalog {
            nodes: vec![Node { name: "My server".into(), subscription: "manual".into(), raw: Value::Null }],
            ..Catalog::default()
        };
        let decision = decide(&strategy, &catalog, &HashMap::new(), "example.com", 443, None);
        assert_eq!(decision.route, Route::Node("My server".into()));
        assert_eq!(decision.chain, ["节点选择", "My server"]);
    }

}
