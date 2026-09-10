use anyhow::{Context, Result};

use crate::catalog::{self, Catalog};
use crate::gfw;
use crate::log;
use crate::paths;
use crate::strategy::{InboundMode, RoutingProfile, Strategy};

pub const CONTROLLER_SECRET: &str = "myproxy-local";

/// Addresses and names that must stay on the local network. These rules are
/// compiled ahead of user rules so a broad user matcher cannot send local
/// traffic through a proxy.
const DEFAULT_DIRECT_RULES: &[&str] = &[
    "DOMAIN,localhost,DIRECT",
    "DOMAIN-SUFFIX,local,DIRECT",
    "DOMAIN-SUFFIX,lan,DIRECT",
    "DOMAIN-SUFFIX,localdomain,DIRECT",
    "DOMAIN-SUFFIX,home.arpa,DIRECT",
    "IP-CIDR,0.0.0.0/32,DIRECT,no-resolve",
    "IP-CIDR,10.0.0.0/8,DIRECT,no-resolve",
    "IP-CIDR,127.0.0.0/8,DIRECT,no-resolve",
    "IP-CIDR,169.254.0.0/16,DIRECT,no-resolve",
    "IP-CIDR,172.16.0.0/12,DIRECT,no-resolve",
    "IP-CIDR,192.168.0.0/16,DIRECT,no-resolve",
    "IP-CIDR,224.0.0.0/4,DIRECT,no-resolve",
    "IP-CIDR,255.255.255.255/32,DIRECT,no-resolve",
    "IP-CIDR6,::/128,DIRECT,no-resolve",
    "IP-CIDR6,::1/128,DIRECT,no-resolve",
    "IP-CIDR6,fc00::/7,DIRECT,no-resolve",
    "IP-CIDR6,fe80::/10,DIRECT,no-resolve",
    "IP-CIDR6,ff00::/8,DIRECT,no-resolve",
];

pub const DNS_LISTEN_PORT: u16 = 1053;

/// Health-check URL for fallback / url-test groups.
const AUTO_GROUP_PROBE_URL: &str = "https://www.gstatic.com/generate_204";
/// Seconds between member probes. 300 left a dead `now` selected for minutes.
const AUTO_GROUP_PROBE_INTERVAL_SECS: i64 = 30;
/// Probe budget in milliseconds so a hung outbound fails the check quickly.
const AUTO_GROUP_PROBE_TIMEOUT_MS: i64 = 3000;
/// Mark a member down after this many failed probes, then use the next alive one.
const AUTO_GROUP_MAX_FAILED_TIMES: i64 = 2;

pub fn controller_port(mixed_port: u16) -> u16 {
    mixed_port.saturating_add(107)
}

pub fn compile(strategy: &Strategy, catalog: &Catalog) -> Result<String> {
    let request = if strategy.system_extension {
        Some(crate::network_extension::try_inbound_plan(strategy)?)
    } else {
        None
    };
    compile_with_inbound_plan(strategy, catalog, request.as_ref())
}

pub fn compile_with_inbound_plan(
    strategy: &Strategy,
    catalog: &Catalog,
    request: Option<&crate::network_extension::EnableRequest>,
) -> Result<String> {
    strategy.validate()?;
    strategy.validate_catalog(catalog)?;
    let root = try_compile_root(strategy, catalog, request)?;
    let compiled = root
        .get("rules")
        .and_then(serde_yaml::Value::as_sequence)
        .map(|rules| rules.len())
        .unwrap_or(0);
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root))?;
    log::info(
        "compile",
        format!(
            "runtime.yaml nodes={} groups={} rule_sets={} compiled_rules={} tun={} se={} mixed={} extension={} routing={}",
            catalog.nodes.len(),
            strategy.groups.len(),
            strategy.rule_sets.len(),
            compiled,
            strategy.tun,
            strategy.system_extension,
            strategy.mixed_mode.as_str(),
            strategy.extension_mode.as_str(),
            strategy.routing_profile.as_str()
        ),
    );
    Ok(yaml)
}

#[cfg(test)]
fn compile_root(strategy: &Strategy, catalog: &Catalog) -> serde_yaml::Mapping {
    let request = if strategy.system_extension {
        Some(crate::network_extension::inbound_plan(strategy))
    } else {
        None
    };
    try_compile_root(strategy, catalog, request.as_ref()).expect("valid fixture inbound plan")
}

fn try_compile_root(
    strategy: &Strategy,
    catalog: &Catalog,
    request: Option<&crate::network_extension::EnableRequest>,
) -> Result<serde_yaml::Mapping> {
    let mut root = serde_yaml::Mapping::new();
    root.insert("allow-lan".into(), false.into());
    root.insert("bind-address".into(), "127.0.0.1".into());
    root.insert("mode".into(), "rule".into());
    root.insert("log-level".into(), "info".into());
    root.insert("find-process-mode".into(), "always".into());
    root.insert("ipv6".into(), true.into());
    root.insert(
        "external-controller".into(),
        format!("127.0.0.1:{}", controller_port(strategy.mixed_port)).into(),
    );
    root.insert("secret".into(), CONTROLLER_SECRET.into());
    insert_inbound_listeners(&mut root, strategy, request)?;
    insert_gfw_sub_rules(&mut root, strategy, request);

    if strategy.system_extension {
        // The DNS proxy provider captures system resolver flows at the
        // Network Extension boundary.  Keep Mihomo's DNS engine available
        // for those relays (and for proxy-server name resolution), but leave
        // packet-level hijacking to the DNS provider; `dns-hijack` only makes
        // sense on the TUN/utun path.
        insert_dns(&mut root, false);
    } else if strategy.tun {
        insert_tun_intercept(&mut root);
    }

    let proxies: Vec<serde_yaml::Value> = catalog.nodes.iter().map(|n| n.raw.clone()).collect();
    root.insert("proxies".into(), serde_yaml::Value::Sequence(proxies));

    let mut groups = Vec::new();
    for group in &strategy.groups {
        let mut members = catalog::resolve_group_members(group, catalog);
        if members.is_empty() {
            log::warn(
                "compile",
                format!("group {} empty, using REJECT sentinel", group.name),
            );
            members.push("REJECT".into());
        }
        let mut item = serde_yaml::Mapping::new();
        item.insert("name".into(), group.name.clone().into());
        let kind = match group.kind.as_str() {
            "url-test" => "url-test",
            "fallback" => "fallback",
            _ => "select",
        };
        item.insert("type".into(), kind.into());
        if kind == "url-test" || kind == "fallback" {
            insert_auto_group_probe(&mut item);
        }
        let proxies: Vec<serde_yaml::Value> =
            members.into_iter().map(serde_yaml::Value::String).collect();
        item.insert("proxies".into(), serde_yaml::Value::Sequence(proxies));
        groups.push(serde_yaml::Value::Mapping(item));
    }
    root.insert("proxy-groups".into(), serde_yaml::Value::Sequence(groups));
    insert_rule_providers(&mut root, strategy);

    let mut rules = Vec::new();
    append_default_direct_rules(&mut rules);
    append_gfw_inlet_rules(&mut rules, strategy, request);
    for set in &strategy.rule_sets {
        for matcher in &set.matchers {
            let Some(condition) = matcher_condition(matcher) else {
                continue;
            };
            let routed = wrap_gfw_condition(&condition, &set.via, matcher.kind.as_str());
            let target = rule_via_target(&set.via, strategy);
            let se_app = matcher.kind == "app" && strategy.system_extension;
            let inlet_names = se_app.then(|| network_extension_inlet_names(request));
            if se_app {
                // A forwarded SE flow belongs to the provider process.
                // Keep process rules for Mixed/TUN without matching that
                // forwarding identity a second time on the private inlet.
                let names = inlet_names.as_deref().expect("se inlet names");
                rules.push(format!(
                    "AND,((NOT,((IN-NAME,{names}))),({routed})),{target}"
                ));
            } else if matcher.kind == "cidr" {
                rules.push(format!("{routed},{target},no-resolve"));
            } else {
                rules.push(format!("{routed},{target}"));
            }
            // gfw:<group> is hit → group, miss → DIRECT. Process GFW
            // inlets already MATCH DIRECT; dest/Mixed process rules need
            // the same rest line so later MATCH/group cannot steal a miss.
            if gfw::gfw_group(&set.via).is_some() && matcher.kind != "cidr" {
                if let Some(names) = inlet_names.as_deref() {
                    rules.push(format!(
                        "AND,((NOT,((IN-NAME,{names}))),({condition})),DIRECT"
                    ));
                } else {
                    rules.push(format!("{condition},DIRECT"));
                }
            }
        }
    }
    if strategy.routing_profile == RoutingProfile::Gfwlist {
        rules.push(format!(
            "RULE-SET,{},{}",
            gfw::PROVIDER,
            default_group(strategy)
        ));
    }
    if strategy.routing_profile == RoutingProfile::Chinadirect {
        rules.push("GEOIP,CN,DIRECT".into());
    }
    rules.push(format!("MATCH,{}", unmatched_target(strategy)));
    let rules: Vec<serde_yaml::Value> = rules.into_iter().map(serde_yaml::Value::String).collect();
    root.insert("rules".into(), serde_yaml::Value::Sequence(rules));
    Ok(root)
}

fn append_default_direct_rules(rules: &mut Vec<String>) {
    rules.extend(DEFAULT_DIRECT_RULES.iter().map(|rule| (*rule).to_string()));
}

fn matcher_condition(matcher: &crate::strategy::Matcher) -> Option<String> {
    match matcher.kind.as_str() {
        "keyword" => Some(format!("DOMAIN-KEYWORD,{}", matcher.value)),
        "suffix" => Some(format!(
            "DOMAIN-SUFFIX,{}",
            matcher.value.trim_start_matches('.')
        )),
        "domain" => {
            if matcher.value.starts_with("*.") {
                Some(format!(
                    "DOMAIN-SUFFIX,{}",
                    matcher.value.trim_start_matches("*.")
                ))
            } else {
                Some(format!("DOMAIN,{}", matcher.value))
            }
        }
        "app" => {
            let value = matcher.value.trim();
            if value.is_empty() {
                return None;
            }
            let kind = if value.contains('/') {
                "PROCESS-PATH"
            } else {
                "PROCESS-NAME"
            };
            let condition = if value.contains(['*', '?']) {
                let pattern = regex::escape(value)
                    .replace(r"\*", ".*")
                    .replace(r"\?", ".");
                format!("{kind}-REGEX,^{pattern}$")
            } else {
                format!("{kind},{value}")
            };
            Some(condition)
        }
        "cidr" => {
            let value = matcher.value.trim();
            if value.is_empty() {
                return None;
            }
            let kind = if value.contains(':') {
                "IP-CIDR6"
            } else {
                "IP-CIDR"
            };
            Some(format!("{kind},{value}"))
        }
        _ => None,
    }
}

fn wrap_gfw_condition(condition: &str, via: &str, kind: &str) -> String {
    if gfw::gfw_group(via).is_some() && kind != "cidr" {
        format!("AND,(({condition}),(RULE-SET,{}))", gfw::PROVIDER)
    } else {
        condition.to_string()
    }
}

fn rule_via_target(via: &str, strategy: &Strategy) -> String {
    if let Some(group) = gfw::gfw_group(via) {
        return via_target(group, strategy);
    }
    via_target(via, strategy)
}

fn gfw_socks_prefix(port: u16) -> String {
    format!("myproxy-network-extension-socks-gfw-{port}")
}

fn gfw_sub_rule_name(port: u16) -> String {
    format!("gfw-{port}")
}

fn gfw_inlet_group(name: &str, strategy: &Strategy) -> String {
    if let Some(group) = gfw::gfw_group(name) {
        via_target(group, strategy)
    } else {
        via_target(name, strategy)
    }
}

fn network_extension_inlet_names(
    plan: Option<&crate::network_extension::EnableRequest>,
) -> String {
    let mut names = vec![
        "myproxy-network-extension-socks-ipv4".to_string(),
        "myproxy-network-extension-socks-ipv6".to_string(),
    ];
    if let Some(plan) = plan {
        for port in &plan.gfw_ports {
            let prefix = gfw_socks_prefix(port.port);
            names.push(format!("{prefix}-ipv4"));
            names.push(format!("{prefix}-ipv6"));
        }
    }
    names.join("/")
}

fn append_gfw_inlet_rules(
    rules: &mut Vec<String>,
    strategy: &Strategy,
    plan: Option<&crate::network_extension::EnableRequest>,
) {
    let Some(plan) = plan else {
        return;
    };
    for port in &plan.gfw_ports {
        let prefix = gfw_socks_prefix(port.port);
        let names = format!("{prefix}-ipv4/{prefix}-ipv6");
        let group = gfw_inlet_group(&port.name, strategy);
        rules.push(format!(
            "AND,((IN-NAME,{names}),(RULE-SET,{})),{group}",
            gfw::PROVIDER
        ));
        rules.push(format!("IN-NAME,{names},DIRECT"));
    }
}

fn insert_gfw_sub_rules(
    root: &mut serde_yaml::Mapping,
    strategy: &Strategy,
    plan: Option<&crate::network_extension::EnableRequest>,
) {
    let Some(plan) = plan else {
        return;
    };
    if plan.gfw_ports.is_empty() {
        return;
    }
    let mut sub = serde_yaml::Mapping::new();
    for port in &plan.gfw_ports {
        let group = gfw_inlet_group(&port.name, strategy);
        let items = vec![
            serde_yaml::Value::String(format!("RULE-SET,{},{group}", gfw::PROVIDER)),
            serde_yaml::Value::String("MATCH,DIRECT".into()),
        ];
        sub.insert(
            gfw_sub_rule_name(port.port).into(),
            serde_yaml::Value::Sequence(items),
        );
    }
    root.insert("sub-rules".into(), serde_yaml::Value::Mapping(sub));
}

fn needs_gfw_provider(strategy: &Strategy) -> bool {
    strategy.routing_profile == RoutingProfile::Gfwlist
        || strategy
            .rule_sets
            .iter()
            .any(|set| gfw::gfw_group(&set.via).is_some())
}

fn insert_rule_providers(root: &mut serde_yaml::Mapping, strategy: &Strategy) {
    if !needs_gfw_provider(strategy) {
        return;
    }
    if let Err(err) = paths::ruleset_dir() {
        log::warn("compile", format!("ruleset dir: {err:#}"));
    }
    let mut provider = serde_yaml::Mapping::new();
    provider.insert("type".into(), "http".into());
    provider.insert("behavior".into(), "domain".into());
    provider.insert("url".into(), gfw::LIST_URL.into());
    provider.insert("path".into(), gfw::RULESET_REL.into());
    provider.insert("interval".into(), 86400.into());
    provider.insert("proxy".into(), "DIRECT".into());
    let mut providers = serde_yaml::Mapping::new();
    providers.insert(gfw::PROVIDER.into(), serde_yaml::Value::Mapping(provider));
    root.insert(
        "rule-providers".into(),
        serde_yaml::Value::Mapping(providers),
    );
}

fn insert_auto_group_probe(item: &mut serde_yaml::Mapping) {
    item.insert("url".into(), AUTO_GROUP_PROBE_URL.into());
    item.insert("interval".into(), AUTO_GROUP_PROBE_INTERVAL_SECS.into());
    item.insert("timeout".into(), AUTO_GROUP_PROBE_TIMEOUT_MS.into());
    item.insert("lazy".into(), false.into());
    item.insert(
        "max-failed-times".into(),
        AUTO_GROUP_MAX_FAILED_TIMES.into(),
    );
}

fn yaml_strings(items: &[&str]) -> serde_yaml::Value {
    serde_yaml::Value::Sequence(
        items
            .iter()
            .map(|s| serde_yaml::Value::String((*s).into()))
            .collect(),
    )
}

/// Configure Mihomo's DNS engine.  TUN additionally asks Mihomo to hijack
/// packets sent to arbitrary resolvers; the Network Extension DNS provider
/// performs that interception on the system resolver path, so SE omits the
/// `dns-hijack` option while sharing the resolver configuration.
fn insert_dns(root: &mut serde_yaml::Mapping, hijack: bool) {
    let mut dns = serde_yaml::Mapping::new();
    dns.insert("enable".into(), true.into());
    dns.insert(
        "listen".into(),
        format!("127.0.0.1:{DNS_LISTEN_PORT}").into(),
    );
    dns.insert("ipv6".into(), true.into());
    dns.insert("enhanced-mode".into(), "fake-ip".into());
    dns.insert("fake-ip-range".into(), "198.18.0.1/16".into());
    dns.insert(
        "fake-ip-filter".into(),
        yaml_strings(&["*.lan", "*.local", "localhost"]),
    );
    dns.insert(
        "default-nameserver".into(),
        yaml_strings(&["8.8.8.8", "1.1.1.1"]),
    );
    dns.insert("nameserver".into(), yaml_strings(&["1.1.1.1", "8.8.8.8"]));
    dns.insert(
        "proxy-server-nameserver".into(),
        yaml_strings(&["8.8.8.8", "1.1.1.1"]),
    );
    root.insert("dns".into(), serde_yaml::Value::Mapping(dns));

    if hijack {
        // `dns-hijack` belongs to the TUN mapping, rather than `dns`.
        if let Some(serde_yaml::Value::Mapping(tun)) = root.get_mut("tun") {
            tun.insert(
                "dns-hijack".into(),
                yaml_strings(&["any:53", "tcp://any:53"]),
            );
        }
    }
}

fn insert_tun_intercept(root: &mut serde_yaml::Mapping) {
    let mut tun = serde_yaml::Mapping::new();
    tun.insert("enable".into(), true.into());
    tun.insert("stack".into(), "system".into());
    tun.insert("auto-route".into(), true.into());
    tun.insert("strict-route".into(), true.into());
    tun.insert("auto-detect-interface".into(), true.into());
    root.insert("tun".into(), serde_yaml::Value::Mapping(tun));

    insert_dns(root, true);

    let mut http = serde_yaml::Mapping::new();
    http.insert(
        "ports".into(),
        serde_yaml::Value::Sequence(vec![
            80.into(),
            serde_yaml::Value::String("8080-8880".into()),
        ]),
    );
    let mut tls = serde_yaml::Mapping::new();
    tls.insert(
        "ports".into(),
        serde_yaml::Value::Sequence(vec![443.into(), 8443.into()]),
    );
    let mut quic = serde_yaml::Mapping::new();
    quic.insert(
        "ports".into(),
        serde_yaml::Value::Sequence(vec![443.into(), 8443.into()]),
    );
    let mut sniff = serde_yaml::Mapping::new();
    sniff.insert("HTTP".into(), serde_yaml::Value::Mapping(http));
    sniff.insert("TLS".into(), serde_yaml::Value::Mapping(tls));
    sniff.insert("QUIC".into(), serde_yaml::Value::Mapping(quic));

    let mut sniffer = serde_yaml::Mapping::new();
    sniffer.insert("enable".into(), true.into());
    sniffer.insert("override-destination".into(), true.into());
    sniffer.insert("force-dns-mapping".into(), true.into());
    sniffer.insert("parse-pure-ip".into(), true.into());
    sniffer.insert("sniff".into(), serde_yaml::Value::Mapping(sniff));
    root.insert("sniffer".into(), serde_yaml::Value::Mapping(sniffer));
}

pub fn network_extension_socks_port(mixed_port: u16) -> u16 {
    let port = mixed_port.saturating_add(1);
    if port == controller_port(mixed_port) {
        port.saturating_add(1)
    } else {
        port
    }
}

pub fn inbound_proxy(mode: InboundMode, strategy: &Strategy) -> Option<String> {
    match mode {
        InboundMode::Rule => None,
        InboundMode::Proxy => Some(default_group(strategy).to_string()),
        InboundMode::Global => Some("GLOBAL".into()),
        InboundMode::Direct => Some("DIRECT".into()),
    }
}

fn insert_inbound_listeners(
    root: &mut serde_yaml::Mapping,
    strategy: &Strategy,
    request: Option<&crate::network_extension::EnableRequest>,
) -> Result<()> {
    let mut listeners = Vec::new();
    push_mixed_listener(&mut listeners, strategy);
    if strategy.system_extension {
        let request = request.context("missing Network Extension listener plan")?;
        append_network_extension_listeners(&mut listeners, strategy, request);
    }
    root.insert("listeners".into(), serde_yaml::Value::Sequence(listeners));
    Ok(())
}

fn push_mixed_listener(listeners: &mut Vec<serde_yaml::Value>, strategy: &Strategy) {
    let mut item = serde_yaml::Mapping::new();
    item.insert("name".into(), "myproxy-mixed".into());
    item.insert("type".into(), "mixed".into());
    item.insert("listen".into(), "127.0.0.1".into());
    item.insert("port".into(), strategy.mixed_port.into());
    if let Some(proxy) = inbound_proxy(strategy.mixed_mode, strategy) {
        item.insert("proxy".into(), proxy.into());
    }
    listeners.push(serde_yaml::Value::Mapping(item));
}

fn append_network_extension_listeners(
    listeners: &mut Vec<serde_yaml::Value>,
    strategy: &Strategy,
    plan: &crate::network_extension::EnableRequest,
) {
    let outbound = inbound_proxy(strategy.extension_mode, strategy);
    push_socks_pair(
        listeners,
        "myproxy-network-extension-socks",
        plan.socks_port,
        outbound.as_deref(),
        &plan.username,
        &plan.password,
        None,
    );
    for group in &plan.group_ports {
        push_socks_pair(
            listeners,
            &format!("myproxy-network-extension-socks-route-{}", group.port),
            group.port,
            Some(group.name.as_str()),
            &plan.username,
            &plan.password,
            None,
        );
    }
    for group in &plan.gfw_ports {
        push_socks_pair(
            listeners,
            &gfw_socks_prefix(group.port),
            group.port,
            None,
            &plan.username,
            &plan.password,
            Some(&gfw_sub_rule_name(group.port)),
        );
    }
}

fn push_socks_pair(
    listeners: &mut Vec<serde_yaml::Value>,
    name_prefix: &str,
    port: u16,
    outbound: Option<&str>,
    username: &str,
    password: &str,
    rule: Option<&str>,
) {
    for (suffix, host) in [("ipv4", "127.0.0.1"), ("ipv6", "::1")] {
        let mut item = serde_yaml::Mapping::new();
        item.insert("name".into(), format!("{name_prefix}-{suffix}").into());
        item.insert("type".into(), "socks".into());
        item.insert("listen".into(), host.into());
        item.insert("port".into(), port.into());
        item.insert("udp".into(), true.into());
        if let Some(proxy) = outbound {
            item.insert("proxy".into(), proxy.into());
        }
        if let Some(rule) = rule {
            item.insert("rule".into(), rule.to_string().into());
        }
        let mut user = serde_yaml::Mapping::new();
        user.insert("username".into(), username.into());
        user.insert("password".into(), password.into());
        item.insert(
            "users".into(),
            serde_yaml::Value::Sequence(vec![serde_yaml::Value::Mapping(user)]),
        );
        listeners.push(serde_yaml::Value::Mapping(item));
    }
}

pub fn via_target(via: &str, strategy: &Strategy) -> String {
    let via = via.trim();
    if let Some(group) = gfw::gfw_group(via) {
        return via_target(group, strategy);
    }
    match via.to_ascii_lowercase().as_str() {
        "direct" => "DIRECT".into(),
        "reject" => "REJECT".into(),
        _ => {
            if let Some(name) = via.strip_prefix("node:") {
                name.to_string()
            } else if let Some(name) = via.strip_prefix("group:") {
                resolve_group_target(name, strategy)
            } else {
                resolve_group_target(via, strategy)
            }
        }
    }
}

fn resolve_group_target(via: &str, strategy: &Strategy) -> String {
    if let Some(group) = strategy
        .groups
        .iter()
        .find(|group| group.name.eq_ignore_ascii_case(via))
    {
        return group.name.clone();
    }
    if via.eq_ignore_ascii_case("default") || via.eq_ignore_ascii_case("proxy") {
        return default_group(strategy).to_string();
    }
    via.to_string()
}

pub fn unmatched_target(strategy: &Strategy) -> String {
    match strategy.routing_profile {
        RoutingProfile::Allowlist | RoutingProfile::Gfwlist => "DIRECT".into(),
        RoutingProfile::Group | RoutingProfile::Chinadirect => {
            let via = strategy.unmatched_via.trim();
            if via.is_empty() || via.eq_ignore_ascii_case("direct") {
                if strategy.routing_profile == RoutingProfile::Chinadirect {
                    default_group(strategy).to_string()
                } else {
                    "DIRECT".into()
                }
            } else {
                via_target(via, strategy)
            }
        }
    }
}

pub fn unmatched_is_direct(strategy: &Strategy) -> bool {
    unmatched_target(strategy).eq_ignore_ascii_case("DIRECT")
}

pub fn default_group(strategy: &Strategy) -> &str {
    strategy.default_group_name()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::strategy::{Group, InboundMode, RoutingProfile, Strategy};

    fn strategy_with_groups(names: &[&str]) -> Strategy {
        let mut strategy = Strategy::default();
        strategy.groups = names
            .iter()
            .map(|name| Group::all_nodes((*name).into(), "select".into()))
            .collect();
        strategy
    }

    #[test]
    fn via_target_resolves_default_to_stored_group_name() {
        let strategy = strategy_with_groups(&["Default", "Telegram"]);
        assert_eq!(via_target("default", &strategy), "Default");
        assert_eq!(via_target("Default", &strategy), "Default");
        assert_eq!(via_target("PROXY", &strategy), "Default");
        assert_eq!(via_target("proxy", &strategy), "Default");
        assert_eq!(via_target("group:default", &strategy), "Default");
    }

    #[test]
    fn via_target_resolves_proxy_alias_when_group_is_proxy() {
        let strategy = strategy_with_groups(&["PROXY", "Telegram"]);
        assert_eq!(via_target("default", &strategy), "PROXY");
        assert_eq!(via_target("PROXY", &strategy), "PROXY");
    }

    #[test]
    fn via_target_preserves_direct_reject_and_named_groups() {
        let strategy = strategy_with_groups(&["Default", "AI Proxy"]);
        assert_eq!(via_target("direct", &strategy), "DIRECT");
        assert_eq!(via_target("REJECT", &strategy), "REJECT");
        assert_eq!(via_target("ai proxy", &strategy), "AI Proxy");
        assert_eq!(via_target("node:Some Node", &strategy), "Some Node");
        assert_eq!(via_target("gfw:Default", &strategy), "Default");
        assert_eq!(via_target("gfwlist:AI Proxy", &strategy), "AI Proxy");
    }

    #[test]
    fn default_group_prefers_proxy_or_default_name() {
        let strategy = strategy_with_groups(&["AI Proxy", "Default"]);
        assert_eq!(default_group(&strategy), "Default");
    }

    #[test]
    fn unmatched_target_defaults_to_direct() {
        let strategy = strategy_with_groups(&["Default", "AI Proxy"]);
        assert_eq!(unmatched_target(&strategy), "DIRECT");
        assert!(unmatched_is_direct(&strategy));
    }

    #[test]
    fn unmatched_target_can_be_direct_or_a_named_group() {
        let mut strategy = strategy_with_groups(&["Default", "AI Proxy"]);
        strategy.routing_profile = RoutingProfile::Group;
        strategy.unmatched_via = "DIRECT".into();
        assert_eq!(unmatched_target(&strategy), "DIRECT");
        assert!(unmatched_is_direct(&strategy));
        strategy.unmatched_via = "AI Proxy".into();
        assert_eq!(unmatched_target(&strategy), "AI Proxy");
        assert!(!unmatched_is_direct(&strategy));
        strategy.unmatched_via = "default".into();
        assert_eq!(unmatched_target(&strategy), "Default");
    }

    fn rule_strings(root: &serde_yaml::Mapping) -> Vec<&str> {
        root.get("rules")
            .and_then(serde_yaml::Value::as_sequence)
            .expect("rules")
            .iter()
            .map(|item| item.as_str().expect("rule"))
            .collect()
    }

    #[test]
    fn allowlist_has_no_rule_provider() {
        let strategy = Strategy::default();
        let root = compiled(&strategy);
        assert!(!root.contains_key("rule-providers"));
        let rules = rule_strings(&root);
        assert!(rules
            .iter()
            .any(|rule| rule.starts_with("DOMAIN-SUFFIX,telegram.org,")));
        assert!(!rules.iter().any(|rule| rule.starts_with("RULE-SET,")));
        assert_eq!(rules.last().copied(), Some("MATCH,DIRECT"));
    }

    #[test]
    fn chinadirect_emits_geoip_then_match_default_group() {
        let mut strategy = Strategy::default();
        strategy.routing_profile = RoutingProfile::Chinadirect;
        strategy.unmatched_via = "PROXY".into();
        let root = compiled(&strategy);
        let rules = rule_strings(&root);
        let geo = rules
            .iter()
            .position(|rule| *rule == "GEOIP,CN,DIRECT")
            .expect("geoip");
        let matched = rules
            .iter()
            .position(|rule| *rule == "MATCH,PROXY")
            .expect("match");
        assert!(geo < matched);
        assert!(!root.contains_key("rule-providers"));
    }

    #[test]
    fn gfwlist_emits_provider_and_rule_set_before_match() {
        let mut strategy = Strategy::default();
        strategy.routing_profile = RoutingProfile::Gfwlist;
        strategy.unmatched_via = "PROXY".into();
        let root = compiled(&strategy);
        let provider = root
            .get("rule-providers")
            .and_then(serde_yaml::Value::as_mapping)
            .and_then(|providers| providers.get("gfw"))
            .and_then(serde_yaml::Value::as_mapping)
            .expect("gfw provider");
        assert_eq!(
            provider.get("type").and_then(serde_yaml::Value::as_str),
            Some("http")
        );
        assert_eq!(
            provider.get("behavior").and_then(serde_yaml::Value::as_str),
            Some("domain")
        );
        assert_eq!(
            provider.get("proxy").and_then(serde_yaml::Value::as_str),
            Some("DIRECT")
        );
        let rules = rule_strings(&root);
        let suffix = rules
            .iter()
            .position(|rule| rule.starts_with("DOMAIN-SUFFIX,telegram.org,"))
            .expect("user suffix");
        let set = rules
            .iter()
            .position(|rule| *rule == "RULE-SET,gfw,PROXY")
            .expect("gfw rule-set");
        assert!(suffix < set);
        assert_eq!(rules.last().copied(), Some("MATCH,DIRECT"));
    }

    #[test]
    fn group_profile_has_no_provider_and_matches_group() {
        let mut strategy = Strategy::default();
        strategy.routing_profile = RoutingProfile::Group;
        strategy.unmatched_via = "Telegram".into();
        let root = compiled(&strategy);
        assert!(!root.contains_key("rule-providers"));
        assert_eq!(rule_strings(&root).last().copied(), Some("MATCH,Telegram"));
    }

    #[test]
    fn fallback_and_url_test_probe_next_member_not_direct() {
        let mut strategy = Strategy::default();
        strategy.groups = vec![
            Group::all_nodes("Default".into(), "fallback".into()),
            Group::all_nodes("Auto".into(), "url-test".into()),
            Group::all_nodes("Manual".into(), "select".into()),
        ];
        let catalog = Catalog {
            nodes: vec![crate::catalog::Node {
                name: "n1".into(),
                subscription: "s".into(),
                raw: serde_yaml::Mapping::from_iter([
                    ("name".into(), serde_yaml::Value::String("n1".into())),
                    ("type".into(), serde_yaml::Value::String("ss".into())),
                ])
                .into(),
            }],
            ..Catalog::default()
        };
        let root = compile_root(&strategy, &catalog);
        let groups = root
            .get("proxy-groups")
            .and_then(serde_yaml::Value::as_sequence)
            .expect("proxy-groups");
        let mut seen_auto = 0;
        for group in groups {
            let item = group.as_mapping().expect("group map");
            let name = item
                .get("name")
                .and_then(serde_yaml::Value::as_str)
                .expect("name");
            let kind = item
                .get("type")
                .and_then(serde_yaml::Value::as_str)
                .expect("type");
            if name == "Manual" {
                assert_eq!(kind, "select");
                assert!(!item.contains_key("url"));
                continue;
            }
            assert!(kind == "fallback" || kind == "url-test", "{name} {kind}");
            assert_eq!(
                item.get("url").and_then(serde_yaml::Value::as_str),
                Some(AUTO_GROUP_PROBE_URL)
            );
            assert_eq!(
                item.get("interval").and_then(serde_yaml::Value::as_i64),
                Some(AUTO_GROUP_PROBE_INTERVAL_SECS)
            );
            assert_eq!(
                item.get("timeout").and_then(serde_yaml::Value::as_i64),
                Some(AUTO_GROUP_PROBE_TIMEOUT_MS)
            );
            assert_eq!(
                item.get("lazy").and_then(serde_yaml::Value::as_bool),
                Some(false)
            );
            assert_eq!(
                item.get("max-failed-times")
                    .and_then(serde_yaml::Value::as_i64),
                Some(AUTO_GROUP_MAX_FAILED_TIMES)
            );
            let members = item
                .get("proxies")
                .and_then(serde_yaml::Value::as_sequence)
                .expect("members");
            assert!(
                !members
                    .iter()
                    .any(|member| member.as_str() == Some("DIRECT")),
                "{name} must not degrade to DIRECT"
            );
            seen_auto += 1;
        }
        assert_eq!(seen_auto, 2);
    }

    #[test]
    fn system_extension_dns_engine_has_no_packet_hijack() {
        let mut root = serde_yaml::Mapping::new();
        insert_dns(&mut root, false);

        let dns = root
            .get("dns")
            .and_then(serde_yaml::Value::as_mapping)
            .expect("DNS mapping");
        assert_eq!(dns.get("enable"), Some(&serde_yaml::Value::Bool(true)));
        assert_eq!(
            dns.get("listen"),
            Some(&serde_yaml::Value::String(format!(
                "127.0.0.1:{DNS_LISTEN_PORT}"
            )))
        );
        assert!(!dns.contains_key("dns-hijack"));
        assert!(!root.contains_key("tun"));
    }

    #[test]
    fn tun_dns_engine_hijacks_udp_and_tcp_port_53() {
        let mut root = serde_yaml::Mapping::new();
        insert_tun_intercept(&mut root);

        let tun = root
            .get("tun")
            .and_then(serde_yaml::Value::as_mapping)
            .expect("TUN mapping");
        let hijack = tun
            .get("dns-hijack")
            .and_then(serde_yaml::Value::as_sequence)
            .expect("DNS hijack list");
        assert_eq!(
            hijack,
            &vec![
                serde_yaml::Value::String("any:53".into()),
                serde_yaml::Value::String("tcp://any:53".into()),
            ]
        );
    }

    fn compiled(strategy: &Strategy) -> serde_yaml::Mapping {
        compile_root(strategy, &Catalog::default())
    }

    fn listeners(root: &serde_yaml::Mapping) -> Vec<&serde_yaml::Mapping> {
        root.get("listeners")
            .and_then(serde_yaml::Value::as_sequence)
            .expect("listeners")
            .iter()
            .map(|item| item.as_mapping().expect("listener map"))
            .collect()
    }

    fn listener_name(item: &serde_yaml::Mapping) -> &str {
        item.get("name")
            .and_then(serde_yaml::Value::as_str)
            .expect("listener name")
    }

    fn mixed_listener(root: &serde_yaml::Mapping) -> &serde_yaml::Mapping {
        listeners(root)
            .into_iter()
            .find(|item| listener_name(item) == "myproxy-mixed")
            .expect("myproxy-mixed")
    }

    fn proxy_field(item: &serde_yaml::Mapping) -> Option<&str> {
        item.get("proxy").and_then(serde_yaml::Value::as_str)
    }

    #[test]
    fn inbound_proxy_maps_four_modes() {
        let strategy = strategy_with_groups(&["PROXY", "Telegram"]);
        assert_eq!(inbound_proxy(InboundMode::Rule, &strategy), None);
        assert_eq!(
            inbound_proxy(InboundMode::Proxy, &strategy).as_deref(),
            Some("PROXY")
        );
        assert_eq!(
            inbound_proxy(InboundMode::Global, &strategy).as_deref(),
            Some("GLOBAL")
        );
        assert_eq!(
            inbound_proxy(InboundMode::Direct, &strategy).as_deref(),
            Some("DIRECT")
        );
    }

    #[test]
    fn compile_emits_mixed_listener_without_top_level_mixed_port() {
        let strategy = Strategy::default();
        let root = compiled(&strategy);
        assert!(!root.contains_key("mixed-port"));
        let mixed = mixed_listener(&root);
        assert_eq!(
            mixed.get("type").and_then(serde_yaml::Value::as_str),
            Some("mixed")
        );
        assert_eq!(
            mixed.get("listen").and_then(serde_yaml::Value::as_str),
            Some("127.0.0.1")
        );
        assert_eq!(
            mixed.get("port").and_then(serde_yaml::Value::as_u64),
            Some(u64::from(strategy.mixed_port))
        );
        assert_eq!(proxy_field(mixed), None);
    }

    #[test]
    fn mixed_modes_set_listener_proxy() {
        let cases = [
            (InboundMode::Rule, None),
            (InboundMode::Proxy, Some("PROXY")),
            (InboundMode::Global, Some("GLOBAL")),
            (InboundMode::Direct, Some("DIRECT")),
        ];
        for (mode, expected) in cases {
            let mut strategy = Strategy::default();
            strategy.mixed_mode = mode;
            let root = compiled(&strategy);
            assert_eq!(
                proxy_field(mixed_listener(&root)),
                expected,
                "mixed mode {mode:?}"
            );
        }
    }

    #[test]
    fn system_extension_rule_keeps_group_socks() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        let root = compiled(&strategy);
        let names: Vec<&str> = listeners(&root).into_iter().map(listener_name).collect();
        assert!(names.contains(&"myproxy-mixed"));
        assert!(names
            .iter()
            .any(|name| name.starts_with("myproxy-network-extension-socks-ipv")));
        assert!(
            names
                .iter()
                .any(|name| name.starts_with("myproxy-network-extension-socks-route-")),
            "rule mode should pin process groups: {names:?}"
        );
        let default_socks = listeners(&root)
            .into_iter()
            .find(|item| listener_name(item) == "myproxy-network-extension-socks-ipv4")
            .expect("default se socks");
        assert_eq!(proxy_field(default_socks), None);
    }

    #[test]
    fn system_extension_non_rule_only_default_socks_with_proxy() {
        let cases = [
            (InboundMode::Proxy, Some("PROXY")),
            (InboundMode::Global, Some("GLOBAL")),
            (InboundMode::Direct, Some("DIRECT")),
        ];
        for (mode, expected) in cases {
            let mut strategy = Strategy::default();
            strategy.system_extension = true;
            strategy.extension_mode = mode;
            let root = compiled(&strategy);
            let names: Vec<&str> = listeners(&root).into_iter().map(listener_name).collect();
            assert!(names.contains(&"myproxy-mixed"), "{mode:?}");
            assert!(
                names
                    .iter()
                    .any(|name| *name == "myproxy-network-extension-socks-ipv4"),
                "{mode:?} names={names:?}"
            );
            assert!(
                names
                    .iter()
                    .all(|name| !name.starts_with("myproxy-network-extension-socks-route-")),
                "{mode:?} should not emit group socks: {names:?}"
            );
            let default_socks = listeners(&root)
                .into_iter()
                .find(|item| listener_name(item) == "myproxy-network-extension-socks-ipv4")
                .expect("default se socks");
            assert_eq!(proxy_field(default_socks), expected, "se mode {mode:?}");
        }
    }

    #[test]
    fn dest_gfw_emits_and_rule_set_and_provider() {
        let mut strategy = Strategy::default();
        strategy.rule_sets.push(crate::strategy::RuleSet {
            id: "safari".into(),
            name: "Safari".into(),
            via: "gfw:Default".into(),
            matchers: vec![crate::strategy::Matcher {
                kind: "suffix".into(),
                value: "example.com".into(),
            }],
        });
        let root = compiled(&strategy);
        assert!(root.contains_key("rule-providers"));
        let rules = rule_strings(&root);
        assert!(
            rules.iter().any(|rule| {
                *rule == "AND,((DOMAIN-SUFFIX,example.com),(RULE-SET,gfw)),PROXY"
            }),
            "{rules:?}"
        );
        let hit = rules
            .iter()
            .position(|rule| *rule == "AND,((DOMAIN-SUFFIX,example.com),(RULE-SET,gfw)),PROXY")
            .expect("gfw hit");
        let miss = rules
            .iter()
            .position(|rule| *rule == "DOMAIN-SUFFIX,example.com,DIRECT")
            .expect("gfw miss");
        assert!(hit < miss, "{rules:?}");
        assert!(!rules.iter().any(|rule| rule.starts_with("IN-NAME,")));
    }

    #[test]
    fn process_gfw_emits_inlet_without_proxy() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        if let Some(set) = strategy.rule_sets.first_mut() {
            set.via = "gfw:Default".into();
            set.matchers = vec![crate::strategy::Matcher {
                kind: "app".into(),
                value: "Safari".into(),
            }];
        }
        let plan = crate::network_extension::inbound_plan(&strategy);
        let port = plan.gfw_ports[0].port;
        let root = compiled(&strategy);
        let prefix = format!("myproxy-network-extension-socks-gfw-{port}");
        let ipv4 = format!("{prefix}-ipv4");
        let listener_names: Vec<&str> = listeners(&root).into_iter().map(listener_name).collect();
        assert!(listener_names.contains(&ipv4.as_str()), "{listener_names:?}");
        let inlet = listeners(&root)
            .into_iter()
            .find(|item| listener_name(item) == ipv4)
            .expect("gfw inlet");
        assert_eq!(proxy_field(inlet), None);
        let sub_name = format!("gfw-{port}");
        assert_eq!(
            inlet.get("rule").and_then(serde_yaml::Value::as_str),
            Some(sub_name.as_str())
        );
        let sub = root
            .get("sub-rules")
            .and_then(serde_yaml::Value::as_mapping)
            .and_then(|rules| rules.get(&sub_name))
            .and_then(serde_yaml::Value::as_sequence)
            .expect("gfw sub-rule");
        assert_eq!(
            sub,
            &vec![
                serde_yaml::Value::String("RULE-SET,gfw,PROXY".into()),
                serde_yaml::Value::String("MATCH,DIRECT".into()),
            ]
        );
        let rules = rule_strings(&root);
        let inlet_names = format!("{prefix}-ipv4/{prefix}-ipv6");
        let hit = format!("AND,((IN-NAME,{inlet_names}),(RULE-SET,gfw)),PROXY");
        let miss = format!("IN-NAME,{inlet_names},DIRECT");
        assert!(rules.iter().any(|rule| *rule == hit), "{rules:?}");
        assert!(rules.iter().any(|rule| *rule == miss), "{rules:?}");
    }

    #[test]
    fn dest_gfw_with_se_emits_inlet_without_list() {
        let mut strategy = Strategy::default();
        strategy.system_extension = true;
        strategy.extension_mode = InboundMode::Rule;
        strategy.rule_sets = vec![crate::strategy::RuleSet {
            id: "safari".into(),
            name: "Safari".into(),
            via: "gfw:Default".into(),
            matchers: vec![crate::strategy::Matcher {
                kind: "suffix".into(),
                value: "example.com".into(),
            }],
        }];
        let plan = crate::network_extension::inbound_plan(&strategy);
        assert!(plan.gfw_domains.is_empty());
        assert_eq!(
            plan.dest_rules
                .iter()
                .find(|rule| rule.value == "example.com")
                .map(|rule| rule.via.as_str()),
            Some("gfw:PROXY")
        );
        let port = plan.gfw_ports[0].port;
        let root = compiled(&strategy);
        let prefix = format!("myproxy-network-extension-socks-gfw-{port}");
        let ipv4 = format!("{prefix}-ipv4");
        let names: Vec<&str> = listeners(&root).into_iter().map(listener_name).collect();
        assert!(names.contains(&ipv4.as_str()), "{names:?}");
        let inlet = listeners(&root)
            .into_iter()
            .find(|item| listener_name(item) == ipv4)
            .expect("gfw inlet");
        assert_eq!(proxy_field(inlet), None);
    }

    #[test]
    fn gfwlist_runtime_passes_mihomo_test() {
        let bin = crate::paths::bundled_mihomo();
        if !bin.is_file() {
            return;
        }
        let mut strategy = Strategy::default();
        strategy.routing_profile = RoutingProfile::Gfwlist;
        let root = compiled(&strategy);
        let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root)).expect("yaml");
        let dir = std::env::temp_dir().join(format!(
            "myproxy-gfw-t-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("ruleset")).expect("ruleset dir");
        let yaml_path = dir.join("runtime.yaml");
        std::fs::write(&yaml_path, yaml).expect("write runtime");
        let status = std::process::Command::new(&bin)
            .arg("-d")
            .arg(&dir)
            .arg("-t")
            .arg("-f")
            .arg(&yaml_path)
            .status()
            .expect("mihomo -t");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(status.success(), "mihomo -t failed: {status}");
    }
}
